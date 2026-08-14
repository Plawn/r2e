//! `TenantContext`: per-tenant resources built on other per-tenant resources.
//!
//! The cascade is what makes tenancy composable — a per-tenant API client can be
//! built on the *same* tenant's per-tenant pool without either source knowing how
//! the other is wired. The properties that need pinning: the tenant is carried
//! through unchanged, app-scoped beans stay reachable, a missing `PerTenant`
//! plugin is a named error rather than a panic, and a cycle is reported with the
//! chain instead of overflowing the stack.

use std::sync::Arc;

use r2e_core::{BeanContext, BeanRegistry};
use r2e_tenant::{
    BoxError, BoxFuture, TenantContext, TenantError, TenantId, TenantSource, Tenanted,
    TenantedSettings,
};

use crate::fixtures::{tid, Behaviour, Resource, ScriptedSource};

/// `Derived` is built from the same tenant's `Resource` (`Resource -> Derived`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Derived {
    from: Resource,
    label: String,
}

#[derive(Clone)]
struct DerivedSource;

impl TenantSource<Derived> for DerivedSource {
    fn create<'a>(
        &'a self,
        tenant: &'a TenantId,
        ctx: &'a TenantContext<'a>,
    ) -> BoxFuture<'a, Result<Option<Derived>, BoxError>> {
        Box::pin(async move {
            let base = ctx.get::<Resource>().await?;
            Ok(Some(Derived {
                from: base,
                label: format!("{tenant}-derived"),
            }))
        })
    }
}

/// A resource built from an app-scoped bean instead of another per-tenant one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamped(String);

#[derive(Clone)]
struct AppBeanSource;

impl TenantSource<Stamped> for AppBeanSource {
    fn create<'a>(
        &'a self,
        tenant: &'a TenantId,
        ctx: &'a TenantContext<'a>,
    ) -> BoxFuture<'a, Result<Option<Stamped>, BoxError>> {
        Box::pin(async move {
            let region = ctx.bean::<String>().ok_or("no region bean")?;
            Ok(Some(Stamped(format!("{tenant}@{region}"))))
        })
    }
}

/// Two resources that need each other (`Left -> Right -> Left`).
#[derive(Clone, Debug)]
struct Left(#[allow(dead_code)] String);
#[derive(Clone, Debug)]
struct Right(#[allow(dead_code)] String);

#[derive(Clone)]
struct LeftSource;
#[derive(Clone)]
struct RightSource;

impl TenantSource<Left> for LeftSource {
    fn create<'a>(
        &'a self,
        _tenant: &'a TenantId,
        ctx: &'a TenantContext<'a>,
    ) -> BoxFuture<'a, Result<Option<Left>, BoxError>> {
        Box::pin(async move {
            let right = ctx.get::<Right>().await?;
            Ok(Some(Left(format!("{right:?}"))))
        })
    }
}

impl TenantSource<Right> for RightSource {
    fn create<'a>(
        &'a self,
        _tenant: &'a TenantId,
        ctx: &'a TenantContext<'a>,
    ) -> BoxFuture<'a, Result<Option<Right>, BoxError>> {
        Box::pin(async move {
            let left = ctx.get::<Left>().await?;
            Ok(Some(Right(format!("{left:?}"))))
        })
    }
}

/// A diamond: `Top` needs `Derived` (which needs `Resource`) **and** `Resource`.
#[derive(Debug, Clone)]
struct Top {
    derived: Derived,
    direct: Resource,
}

#[derive(Clone)]
struct TopSource;

impl TenantSource<Top> for TopSource {
    fn create<'a>(
        &'a self,
        _tenant: &'a TenantId,
        ctx: &'a TenantContext<'a>,
    ) -> BoxFuture<'a, Result<Option<Top>, BoxError>> {
        Box::pin(async move {
            let derived = ctx.get::<Derived>().await?;
            let direct = ctx.get::<Resource>().await?;
            Ok(Some(Top { derived, direct }))
        })
    }
}

/// Builds a bean graph of per-tenant maps the way the plugin does: unwired shells
/// go into the graph first (so every map can see every other), then each is wired
/// against the resolved context.
/// Deferred `Tenanted::wire` call, run once the graph has resolved.
type Wiring = Box<dyn FnOnce(&Arc<BeanContext>)>;

struct Graph {
    registry: BeanRegistry,
    wirings: Vec<Wiring>,
}

impl Graph {
    fn new() -> Self {
        Self {
            registry: BeanRegistry::new(),
            wirings: Vec::new(),
        }
    }

    /// Register an app-scoped bean.
    fn bean<T: Clone + Send + Sync + 'static>(mut self, value: T) -> Self {
        self.registry.provide(value);
        self
    }

    /// Register a per-tenant map of `T` backed by `source`.
    fn per_tenant<T, S>(mut self, source: S) -> Self
    where
        T: Clone + Send + Sync + 'static,
        S: TenantSource<T>,
    {
        let map: Tenanted<T> = Tenanted::unwired();
        self.registry.provide(map.clone());
        self.wirings.push(Box::new(move |ctx| {
            map.wire(
                Arc::new(source),
                Arc::clone(ctx),
                TenantedSettings::default(),
                None,
            );
        }));
        self
    }

    async fn build(self) -> Arc<BeanContext> {
        let Self { registry, wirings } = self;
        let context = Arc::new(registry.resolve().await.expect("graph resolves"));
        for wire in wirings {
            wire(&context);
        }
        context
    }
}

/// The map for `T` out of a built graph.
fn map_of<T: Clone + Send + Sync + 'static>(context: &Arc<BeanContext>) -> Tenanted<T> {
    context.get::<Tenanted<T>>()
}

#[tokio::test]
async fn a_resource_is_built_from_the_same_tenants_other_resource() {
    let base_source = ScriptedSource::new();
    let context = Graph::new()
        .per_tenant::<Resource, _>(base_source.clone())
        .per_tenant::<Derived, _>(DerivedSource)
        .build()
        .await;

    let value = map_of::<Derived>(&context)
        .get(&tid("acme"))
        .await
        .expect("cascade resolves");

    assert_eq!(value.label, "acme-derived");
    assert_eq!(
        value.from.tenant, "acme",
        "the cascade must carry the same tenant through"
    );
    assert_eq!(
        base_source.creates(),
        1,
        "the base resource is created lazily, once"
    );

    // The base resource is now cached in its own map, not duplicated inside the
    // derived one.
    assert_eq!(map_of::<Resource>(&context).active().len(), 1);
}

#[tokio::test]
async fn a_source_can_read_app_scoped_beans() {
    let context = Graph::new()
        .bean("eu-west".to_string())
        .per_tenant::<Stamped, _>(AppBeanSource)
        .build()
        .await;

    let value = map_of::<Stamped>(&context).get(&tid("acme")).await.unwrap();
    assert_eq!(value, Stamped("acme@eu-west".into()));
}

#[tokio::test]
async fn a_missing_per_tenant_plugin_names_the_type() {
    // `Derived` cascades into `Resource`, but no `Tenanted<Resource>` is in the
    // graph — the mistake of installing one `PerTenant` and forgetting the other.
    let context = Graph::new()
        .per_tenant::<Derived, _>(DerivedSource)
        .build()
        .await;

    let err = map_of::<Derived>(&context)
        .get(&tid("acme"))
        .await
        .unwrap_err();

    let message = err.to_string();
    assert!(
        message.contains("Resource"),
        "the error must name the missing resource type: {message}"
    );
    assert!(
        message.contains("PerTenant"),
        "the error must name the plugin to add: {message}"
    );
    assert!(err.is_bug());
}

#[tokio::test]
async fn a_cycle_is_reported_with_the_chain() {
    let context = Graph::new()
        .per_tenant::<Left, _>(LeftSource)
        .per_tenant::<Right, _>(RightSource)
        .build()
        .await;

    let err = map_of::<Left>(&context).get(&tid("acme")).await.unwrap_err();

    let message = err.to_string();
    assert!(
        message.contains("cycle"),
        "expected a cycle report, got: {message}"
    );
    assert!(
        message.contains("Left -> Right -> Left"),
        "the chain must be spelled out: {message}"
    );
    assert!(err.is_bug(), "a cycle is a wiring bug, not a bad request");
}

#[tokio::test]
async fn a_diamond_creates_the_shared_resource_once() {
    let base_source = ScriptedSource::new();
    let context = Graph::new()
        .per_tenant::<Resource, _>(base_source.clone())
        .per_tenant::<Derived, _>(DerivedSource)
        .per_tenant::<Top, _>(TopSource)
        .build()
        .await;

    let value = map_of::<Top>(&context)
        .get(&tid("acme"))
        .await
        .expect("diamond resolves");

    assert_eq!(
        value.derived.from, value.direct,
        "both arms of the diamond must see the same underlying resource"
    );
    assert_eq!(
        base_source.creates(),
        1,
        "the shared resource is created once, not once per path"
    );
}

#[tokio::test]
async fn the_context_exposes_the_tenant_and_the_chain() {
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Introspected {
        tenant: String,
        chain: String,
    }

    #[derive(Clone)]
    struct IntrospectingSource;

    impl TenantSource<Introspected> for IntrospectingSource {
        fn create<'a>(
            &'a self,
            _tenant: &'a TenantId,
            ctx: &'a TenantContext<'a>,
        ) -> BoxFuture<'a, Result<Option<Introspected>, BoxError>> {
            Box::pin(async move {
                Ok(Some(Introspected {
                    tenant: ctx.tenant().as_str().to_string(),
                    chain: ctx.chain(),
                }))
            })
        }
    }

    let context = Graph::new()
        .per_tenant::<Introspected, _>(IntrospectingSource)
        .build()
        .await;

    let value = map_of::<Introspected>(&context)
        .get(&tid("acme"))
        .await
        .unwrap();
    assert_eq!(value.tenant, "acme");
    assert_eq!(
        value.chain, "Introspected",
        "a root resolution's chain is just the resource being built"
    );
}

#[tokio::test]
async fn cascade_failures_surface_as_unavailable_on_the_outer_resource() {
    let context = Graph::new()
        .per_tenant::<Resource, _>(ScriptedSource::with_default(Behaviour::Fail(
            "directory down".into(),
        )))
        .per_tenant::<Derived, _>(DerivedSource)
        .build()
        .await;

    let err = map_of::<Derived>(&context)
        .get(&tid("acme"))
        .await
        .unwrap_err();

    assert!(
        matches!(err, TenantError::Unavailable { .. }),
        "the inner failure must surface as Unavailable: {err:?}"
    );
    assert!(err.to_string().contains("directory down"), "{err}");
}

#[tokio::test]
async fn an_unknown_inner_tenant_fails_the_outer_resource() {
    let context = Graph::new()
        .per_tenant::<Resource, _>(ScriptedSource::with_default(Behaviour::Unknown))
        .per_tenant::<Derived, _>(DerivedSource)
        .build()
        .await;

    let err = map_of::<Derived>(&context)
        .get(&tid("ghost"))
        .await
        .unwrap_err();

    // The inner "not provisioned" answer keeps its meaning through the cascade:
    // the tenant does not exist, which is a 404, not a retryable 503.
    assert!(
        matches!(err, TenantError::Unknown(ref t) if t.as_str() == "ghost"),
        "expected the inner Unknown to propagate: {err:?}"
    );
}
