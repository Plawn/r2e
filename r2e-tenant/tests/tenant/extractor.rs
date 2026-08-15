//! `Tenant<T>` and `TenantId` at the call site, through real controllers.
//!
//! These drive an actual router: the point is not that the map works (that is
//! `map.rs`) but that the request path wires up — the tenant is resolved from
//! the request, the *right* tenant's resource is handed to the handler, failures
//! come back with the configured statuses, and the `Option` forms mean "no
//! tenant", never "bad tenant".

use std::sync::Arc;

use r2e_core::http::{Router, StatusCode};
use r2e_core::prelude::*;
use r2e_tenant::{
    HeaderTenantResolver, MissingTenantPolicy, Tenant, TenantId, TenantRouter, TenantStatuses,
    Tenanted, TenantedSettings,
};

use crate::fixtures::{send, Behaviour, Resource, ScriptedSource};

// ── controllers ─────────────────────────────────────────────────────────────

/// The headline shape: a per-tenant resource as a request-scoped field.
#[controller(path = "/orders")]
struct OrderController {
    #[inject(request)]
    db: Tenant<Resource>,
}

#[routes]
impl OrderController {
    #[get("/")]
    async fn list(&self) -> String {
        // `Deref` to the resource, plus the id the resource was built for.
        format!("{}#{}", self.db.tenant, self.db.tenant_id())
    }
}

/// A route that serves both a tenant-scoped and a global view.
#[controller(path = "/maybe")]
struct MaybeController {
    #[inject(request)]
    db: Option<Tenant<Resource>>,
}

#[routes]
impl MaybeController {
    #[get("/")]
    async fn list(&self) -> String {
        match &self.db {
            Some(db) => format!("tenant:{}", db.tenant),
            None => "global".to_string(),
        }
    }
}

/// Just the id — no per-tenant resource involved.
#[controller(path = "/who")]
struct WhoController {
    #[inject(request)]
    tenant: TenantId,
}

#[routes]
impl WhoController {
    #[get("/")]
    async fn who(&self) -> String {
        self.tenant.as_str().to_string()
    }
}

#[controller(path = "/who-maybe")]
struct MaybeWhoController {
    #[inject(request)]
    tenant: Option<TenantId>,
}

#[routes]
impl MaybeWhoController {
    #[get("/")]
    async fn who(&self) -> String {
        match &self.tenant {
            Some(tenant) => tenant.as_str().to_string(),
            None => "none".to_string(),
        }
    }
}

// ── wiring ──────────────────────────────────────────────────────────────────

/// A router over all four controllers, with the given policy and statuses.
async fn app(
    source: ScriptedSource,
    policy: MissingTenantPolicy,
    statuses: TenantStatuses,
) -> Router {
    let map = Tenanted::new(
        Arc::new(source),
        r2e_core::plugin::GraphHandle::default(),
        TenantedSettings {
            statuses,
            ..TenantedSettings::default()
        },
        None,
    );
    let router = TenantRouter::ready(
        Arc::new(HeaderTenantResolver::default()),
        policy,
        statuses,
    );

    r2e_core::AppBuilder::new()
        .provide(router)
        .provide(map)
        .build_state()
        .await
        .register_controller::<OrderController>()
        .register_controller::<MaybeController>()
        .register_controller::<WhoController>()
        .register_controller::<MaybeWhoController>()
        .build()
}

/// The common case: reject requests that carry no tenant, default statuses.
async fn strict_app(source: ScriptedSource) -> Router {
    app(source, MissingTenantPolicy::Reject, TenantStatuses::default()).await
}

// ── tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_handler_gets_the_requesting_tenants_resource() {
    let router = strict_app(ScriptedSource::new()).await;
    let (status, body) = send(router, "/orders", &[("x-tenant-id", "acme")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "acme#acme");
}

#[tokio::test]
async fn two_tenants_get_two_resources() {
    let source = ScriptedSource::new();
    let router = strict_app(source.clone()).await;

    let (_, acme) = send(router.clone(), "/orders", &[("x-tenant-id", "acme")]).await;
    let (_, globex) = send(router.clone(), "/orders", &[("x-tenant-id", "globex")]).await;
    assert_eq!(acme, "acme#acme");
    assert_eq!(globex, "globex#globex");

    // A second request for a known tenant reuses the cached resource.
    let (_, again) = send(router, "/orders", &[("x-tenant-id", "acme")]).await;
    assert_eq!(again, "acme#acme");
    assert_eq!(source.creates(), 2, "one create per tenant, not per request");
}

#[tokio::test]
async fn a_missing_tenant_is_the_missing_status() {
    let router = strict_app(ScriptedSource::new()).await;
    let (status, body) = send(router, "/orders", &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("no tenant"), "{body}");
}

#[tokio::test]
async fn a_malformed_tenant_is_a_400_even_when_the_policy_allows_absence() {
    let router = app(
        ScriptedSource::new(),
        MissingTenantPolicy::Allow,
        TenantStatuses::default(),
    )
    .await;
    // Present but invalid is the resolver's error — `Allow` is about *absence*.
    let (status, _) = send(router, "/maybe", &[("x-tenant-id", "../etc/passwd")]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_unknown_tenant_is_a_404() {
    let source = ScriptedSource::new().on("ghost", Behaviour::Unknown);
    let router = strict_app(source).await;
    let (status, body) = send(router, "/orders", &[("x-tenant-id", "ghost")]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("ghost"), "{body}");
}

#[tokio::test]
async fn an_uncreatable_tenant_is_a_503() {
    let source = ScriptedSource::new().on("acme", Behaviour::Fail("pool refused".into()));
    let router = strict_app(source).await;
    let (status, body) = send(router, "/orders", &[("x-tenant-id", "acme")]).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.contains("pool refused"), "{body}");
}

#[tokio::test]
async fn the_statuses_are_configurable_end_to_end() {
    let source = ScriptedSource::new()
        .on("ghost", Behaviour::Unknown)
        .on("broken", Behaviour::Fail("nope".into()));
    let router = app(
        source,
        MissingTenantPolicy::Reject,
        TenantStatuses {
            missing: StatusCode::UNAUTHORIZED,
            unknown: StatusCode::FORBIDDEN,
            unavailable: StatusCode::BAD_GATEWAY,
        },
    )
    .await;

    let (missing, _) = send(router.clone(), "/orders", &[]).await;
    assert_eq!(missing, StatusCode::UNAUTHORIZED);
    let (unknown, _) = send(router.clone(), "/orders", &[("x-tenant-id", "ghost")]).await;
    assert_eq!(unknown, StatusCode::FORBIDDEN);
    let (unavailable, _) = send(router, "/orders", &[("x-tenant-id", "broken")]).await;
    assert_eq!(unavailable, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn the_optional_form_yields_none_only_for_an_absent_tenant() {
    let source = ScriptedSource::new().on("ghost", Behaviour::Unknown);
    let router = app(
        source,
        MissingTenantPolicy::Allow,
        TenantStatuses::default(),
    )
    .await;

    let (status, body) = send(router.clone(), "/maybe", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "global");

    let (status, body) = send(router.clone(), "/maybe", &[("x-tenant-id", "acme")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "tenant:acme");

    // Present-but-unknown is still an error: `Option` covers "no tenant", never
    // "bad tenant".
    let (status, _) = send(router, "/maybe", &[("x-tenant-id", "ghost")]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_rejecting_policy_makes_even_the_optional_form_fail() {
    let router = strict_app(ScriptedSource::new()).await;
    let (status, _) = send(router, "/maybe", &[]).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "`on-missing = reject` is an app-wide rule; `Option` does not opt out of it"
    );
}

#[tokio::test]
async fn the_id_extractor_needs_no_per_tenant_resource() {
    let source = ScriptedSource::new();
    let router = strict_app(source.clone()).await;

    let (status, body) = send(router.clone(), "/who", &[("x-tenant-id", "acme")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "acme");
    assert_eq!(
        source.creates(),
        0,
        "asking who the tenant is must not provision anything"
    );

    let (status, _) = send(router, "/who", &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_optional_id_extractor_yields_none_without_a_tenant() {
    let router = app(
        ScriptedSource::new(),
        MissingTenantPolicy::Allow,
        TenantStatuses::default(),
    )
    .await;

    let (_, body) = send(router.clone(), "/who-maybe", &[]).await;
    assert_eq!(body, "none");
    let (_, body) = send(router, "/who-maybe", &[("x-tenant-id", "acme")]).await;
    assert_eq!(body, "acme");
}

/// Two request-scoped tenancy fields on one route: without the
/// `parts.extensions` memo this would resolve the tenant twice.
#[controller(path = "/both")]
struct BothController {
    #[inject(request)]
    db: Tenant<Resource>,
    #[inject(request)]
    tenant: TenantId,
}

#[routes]
impl BothController {
    #[get("/")]
    async fn both(&self) -> String {
        format!("{}/{}", self.db.tenant, self.tenant)
    }
}

#[tokio::test]
async fn the_resolver_runs_once_per_request() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = calls.clone();
    let router = r2e_tenant::FnTenantResolver::new(move |req: &r2e_core::request_head::RequestHead<'_>| {
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(req
            .header("x-tenant-id")
            .and_then(|raw| TenantId::parse(raw).ok()))
    });

    let map = Tenanted::new(
        Arc::new(ScriptedSource::new()),
        r2e_core::plugin::GraphHandle::default(),
        TenantedSettings::default(),
        None,
    );
    let app = r2e_core::AppBuilder::new()
        .provide(TenantRouter::ready(
            Arc::new(router),
            MissingTenantPolicy::Reject,
            TenantStatuses::default(),
        ))
        .provide(map)
        .build_state()
        .await
        .register_controller::<BothController>()
        .build();

    let (status, body) = send(app, "/both", &[("x-tenant-id", "acme")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "acme/acme");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the resolved tenant must be memoized for the rest of the request"
    );
}

/// Bridge-overlap invariant pin (see `r2e-core/src/extract.rs` module docs):
/// `Tenant<T>` and `TenantId` must each have exactly ONE extraction route — the
/// `ViaBean`-marked one. Adding a generic axum `FromRequestParts` impl for
/// either would create a second route and turn every controller using them into
/// an opaque `E0283` at `register_controller()`; this probe fails first, with
/// the competing impls listed.
#[test]
fn tenant_extraction_routes_are_unambiguous() {
    use r2e_core::extract::assert_unambiguous_extractor;
    use r2e_core::type_list::{HCons, HNil};

    type S = HCons<TenantRouter, HCons<Tenanted<Resource>, HNil>>;

    assert_unambiguous_extractor::<S, Tenant<Resource>, _>();
    assert_unambiguous_extractor::<S, Option<Tenant<Resource>>, _>();
    assert_unambiguous_extractor::<S, TenantId, _>();
    assert_unambiguous_extractor::<S, Option<TenantId>, _>();
}
