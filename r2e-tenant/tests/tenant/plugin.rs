//! The two plugins, through a real `AppBuilder`.
//!
//! What the plugins own is the *install/configure split*: the beans controllers
//! compile against (`TenantRouter`, `Tenanted<T>`) must exist before the graph is
//! resolved, while the resolver and the source are ordinary beans that only exist
//! after. These tests pin both halves — that the shells get filled, that config
//! and builder settings compose in the documented order, and that
//! `tenancy.enabled: false` still boots.

use std::sync::Arc;
use std::time::Duration;

use r2e_core::http::StatusCode;
use r2e_core::prelude::*;
use r2e_core::{AppBuilder, R2eConfig};
use r2e_tenant::{
    BoxError, BoxFuture, HeaderTenantResolver, MissingTenantPolicy, PerTenant, Tenancy, Tenant,
    TenantContext, TenantId, TenantRouter, TenantSource, TenantStatuses, Tenanted,
    DEFAULT_MAX_ACTIVE,
};

use crate::fixtures::{send, tid, Behaviour, Resource, ScriptedSource};

/// A controller exercising the wired beans end-to-end.
#[controller(path = "/orders")]
struct OrderController {
    #[inject(request)]
    db: Tenant<Resource>,
}

#[routes]
impl OrderController {
    #[get("/")]
    async fn list(&self) -> String {
        self.db.tenant.clone()
    }
}

/// A controller that tolerates a tenant-less request.
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

/// A `TenancyConfig` YAML fragment as an `R2eConfig`.
fn config(yaml: &str) -> R2eConfig {
    R2eConfig::from_yaml_str(yaml).expect("valid yaml")
}

// ── Tenancy ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_tenancy_plugin_builds_a_wired_router() {
    let builder = AppBuilder::new()
        .provide(HeaderTenantResolver::default())
        .plugin(Tenancy::resolver::<HeaderTenantResolver>())
        .build_state()
        .await;

    let router = builder.bean_context().get::<TenantRouter>();
    assert!(
        router.is_enabled(),
        "build resolves the resolver bean and wires the router in one step"
    );
    assert_eq!(router.policy(), MissingTenantPolicy::Reject, "fail closed");
    assert_eq!(router.statuses(), TenantStatuses::default());
}

#[tokio::test]
async fn the_policy_comes_from_config() {
    let builder = AppBuilder::new()
        .override_config(config("tenancy:\n  on-missing: allow\n"))
        .load_config::<()>()
        .provide(HeaderTenantResolver::default())
        .plugin(Tenancy::resolver::<HeaderTenantResolver>())
        .build_state()
        .await;

    assert_eq!(
        builder.bean_context().get::<TenantRouter>().policy(),
        MissingTenantPolicy::Allow
    );
}

#[tokio::test]
async fn a_builder_override_beats_the_file() {
    // The documented precedence: builder > file > built-in default.
    let builder = AppBuilder::new()
        .override_config(config("tenancy:\n  on-missing: allow\n"))
        .load_config::<()>()
        .provide(HeaderTenantResolver::default())
        .plugin(Tenancy::resolver::<HeaderTenantResolver>().require_tenant())
        .build_state()
        .await;

    assert_eq!(
        builder.bean_context().get::<TenantRouter>().policy(),
        MissingTenantPolicy::Reject
    );
}

#[tokio::test]
async fn the_statuses_come_from_config() {
    let builder = AppBuilder::new()
        .override_config(config(
            "tenancy:\n  missing-status: 401\n  unknown-status: 403\n  unavailable-status: 502\n",
        ))
        .load_config::<()>()
        .provide(HeaderTenantResolver::default())
        .plugin(Tenancy::resolver::<HeaderTenantResolver>())
        .build_state()
        .await;

    assert_eq!(
        builder.bean_context().get::<TenantRouter>().statuses(),
        TenantStatuses {
            missing: StatusCode::UNAUTHORIZED,
            unknown: StatusCode::FORBIDDEN,
            unavailable: StatusCode::BAD_GATEWAY,
        }
    );
}

#[tokio::test]
#[should_panic(expected = "invalid `tenancy.on-missing`")]
async fn a_typo_in_the_policy_fails_at_boot() {
    // A fail-closed switch must never be silently mis-set by a typo.
    let _ = AppBuilder::new()
        .override_config(config("tenancy:\n  on-missing: rejct\n"))
        .load_config::<()>()
        .provide(HeaderTenantResolver::default())
        .plugin(Tenancy::resolver::<HeaderTenantResolver>())
        .build_state()
        .await;
}

#[tokio::test]
async fn disabling_tenancy_still_boots_and_serves() {
    // `tenancy.enabled: false` skips `configure`, so the *disabled* router has to
    // come out of `install` — otherwise turning tenancy off would look like a
    // wiring bug (500) to every route.
    let router = AppBuilder::new()
        .override_config(config("tenancy:\n  enabled: false\n"))
        .load_config::<()>()
        .provide(HeaderTenantResolver::default())
        .provide(ScriptedSource::new())
        .plugin(Tenancy::resolver::<HeaderTenantResolver>())
        .plugin(PerTenant::<Resource>::from::<ScriptedSource>())
        .build_state()
        .await
        .register_controller::<OrderController>()
        .register_controller::<MaybeController>()
        .build();

    // Nothing resolves: the optional route serves its global view...
    let (status, body) = send(router.clone(), "/maybe", &[("x-tenant-id", "acme")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "global");

    // ...and a route that *requires* a tenant reports one missing, not a bug.
    let (status, body) = send(router, "/orders", &[("x-tenant-id", "acme")]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("no tenant"), "{body}");
}

#[tokio::test]
async fn a_disabled_tenancy_honours_the_configured_missing_status() {
    let builder = AppBuilder::new()
        .override_config(config(
            "tenancy:\n  enabled: false\n  missing-status: 409\n",
        ))
        .load_config::<()>()
        .provide(HeaderTenantResolver::default())
        .plugin(Tenancy::resolver::<HeaderTenantResolver>())
        .build_state()
        .await;

    let router = builder.bean_context().get::<TenantRouter>();
    assert!(!router.is_enabled());
    assert_eq!(router.statuses().missing, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_resolver_bean_can_have_its_own_dependencies() {
    // The reason the resolver is named as a type and resolved after the graph:
    // it is an ordinary bean.
    #[derive(Clone)]
    struct HeaderName(String);

    #[derive(Clone)]
    struct ConfiguredResolver {
        header: HeaderName,
    }

    impl r2e_tenant::SyncTenantResolver for ConfiguredResolver {
        fn resolve_sync(
            &self,
            req: &r2e_core::request_head::RequestHead<'_>,
        ) -> Result<Option<TenantId>, r2e_core::HttpError> {
            Ok(req
                .header(&self.header.0)
                .and_then(|raw| TenantId::parse(raw).ok()))
        }
    }

    let builder = AppBuilder::new()
        .provide(HeaderName("x-org".into()))
        .provide(ConfiguredResolver {
            header: HeaderName("x-org".into()),
        })
        .provide(ScriptedSource::new())
        .plugin(Tenancy::resolver::<ConfiguredResolver>())
        .plugin(PerTenant::<Resource>::from::<ScriptedSource>())
        .build_state()
        .await;

    let router = builder.register_controller::<OrderController>().build();
    let (status, body) = send(router, "/orders", &[("x-org", "acme")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "acme");
}

// ── PerTenant ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_per_tenant_plugin_fills_the_map_shell() {
    let builder = AppBuilder::new()
        .provide(ScriptedSource::new())
        .plugin(PerTenant::<Resource>::from::<ScriptedSource>())
        .build_state()
        .await;

    let map = builder.bean_context().get::<Tenanted<Resource>>();
    let resource = map.get(&tid("acme")).await.expect("the map is wired");
    assert_eq!(resource.tenant, "acme");
}

#[tokio::test]
async fn map_settings_default_to_the_config_section() {
    let builder = AppBuilder::new()
        .override_config(config(
            "tenancy:\n  max-active: 7\n  idle-ttl: 30s\n  create-timeout: 0\n",
        ))
        .load_config::<()>()
        .provide(ScriptedSource::new())
        .plugin(PerTenant::<Resource>::from::<ScriptedSource>())
        .build_state()
        .await;

    let settings = builder
        .bean_context()
        .get::<Tenanted<Resource>>()
        .settings();
    assert_eq!(settings.max_active, 7);
    assert_eq!(settings.idle_ttl, Some(Duration::from_secs(30)));
    assert_eq!(
        settings.create_timeout, None,
        "`0` is how a duration is switched off"
    );
}

#[tokio::test]
async fn builder_settings_win_over_the_config_section() {
    let builder = AppBuilder::new()
        .override_config(config("tenancy:\n  max-active: 7\n  idle-ttl: 30s\n"))
        .load_config::<()>()
        .provide(ScriptedSource::new())
        .plugin(
            PerTenant::<Resource>::from::<ScriptedSource>()
                .max_active(3)
                .keep_forever()
                .create_timeout(Duration::from_millis(250))
                .negative_ttl(Duration::ZERO),
        )
        .build_state()
        .await;

    let settings = builder
        .bean_context()
        .get::<Tenanted<Resource>>()
        .settings();
    assert_eq!(settings.max_active, 3);
    assert_eq!(settings.idle_ttl, None);
    assert_eq!(settings.create_timeout, Some(Duration::from_millis(250)));
    assert_eq!(settings.negative_ttl, None);
}

#[tokio::test]
async fn map_settings_fall_back_to_the_built_in_defaults() {
    let builder = AppBuilder::new()
        .provide(ScriptedSource::new())
        .plugin(PerTenant::<Resource>::from::<ScriptedSource>())
        .build_state()
        .await;

    let settings = builder
        .bean_context()
        .get::<Tenanted<Resource>>()
        .settings();
    assert_eq!(settings.max_active, DEFAULT_MAX_ACTIVE);
    assert!(settings.idle_ttl.is_some());
    assert!(settings.create_timeout.is_some());
}

#[tokio::test]
async fn fallback_to_default_serves_the_app_scoped_bean() {
    // The migration shape: everything not yet provisioned lands on the shared
    // resource instead of a 404.
    let shared = Resource {
        tenant: "shared".into(),
        generation: 999,
    };
    let source = ScriptedSource::with_default(Behaviour::Unknown);

    let router = AppBuilder::new()
        .override_config(config("tenancy:\n  on-missing: allow\n"))
        .load_config::<()>()
        .provide(HeaderTenantResolver::default())
        .provide(shared)
        .provide(source.clone())
        .plugin(Tenancy::resolver::<HeaderTenantResolver>())
        .plugin(PerTenant::<Resource>::from::<ScriptedSource>().fallback_to_default())
        .build_state()
        .await
        .register_controller::<MaybeController>()
        .build();

    let (status, body) = send(router, "/maybe", &[("x-tenant-id", "ghost")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "tenant:shared", "an unknown tenant gets the default");
    assert!(
        source.disposals().is_empty(),
        "the app-scoped default is never disposed by the tenancy layer"
    );
}

#[tokio::test]
async fn eager_tenants_are_accepted_and_do_not_block_boot() {
    // The warm-up itself runs on `on_serve`; what must hold at build time is that
    // declaring it changes nothing about booting (and that a broken tenant in the
    // list is not fatal).
    let builder = AppBuilder::new()
        .provide(ScriptedSource::with_default(Behaviour::Fail("down".into())))
        .plugin(
            PerTenant::<Resource>::from::<ScriptedSource>()
                .eager([tid("acme"), tid("globex")]),
        )
        .build_state()
        .await;

    assert!(builder
        .bean_context()
        .get::<Tenanted<Resource>>()
        .active()
        .is_empty());
}

#[tokio::test]
async fn per_tenant_resources_cascade_through_the_plugins() {
    // Two `PerTenant` plugins, one source reading the other's map out of the
    // graph. This is the reason the map retains a `GraphHandle` on the resolved
    // graph: at request time it must see every sibling map, whatever order the
    // plugins were built in.
    #[derive(Clone, Debug)]
    struct Report {
        from: String,
    }

    #[derive(Clone)]
    struct Reports;

    impl TenantSource<Report> for Reports {
        fn create<'a>(
            &'a self,
            _tenant: &'a TenantId,
            ctx: &'a TenantContext<'a>,
        ) -> BoxFuture<'a, Result<Option<Report>, BoxError>> {
            Box::pin(async move {
                let base = ctx.get::<Resource>().await?;
                Ok(Some(Report { from: base.tenant }))
            })
        }
    }

    let builder = AppBuilder::new()
        .provide(ScriptedSource::new())
        .provide(Reports)
        .plugin(PerTenant::<Resource>::from::<ScriptedSource>())
        .plugin(PerTenant::<Report>::from::<Reports>())
        .build_state()
        .await;

    let report = builder
        .bean_context()
        .get::<Tenanted<Report>>()
        .get(&tid("acme"))
        .await
        .expect("the cascade sees the other plugin's map");
    assert_eq!(report.from, "acme");
}

#[tokio::test]
async fn a_source_bean_reads_app_scoped_beans_through_the_context() {
    // `TenantContext::bean` must see the beans the builder resolved, including
    // ones provided after the plugin.
    #[derive(Clone)]
    struct Region(String);

    #[derive(Clone)]
    struct RegionalSource;

    impl TenantSource<Resource> for RegionalSource {
        fn create<'a>(
            &'a self,
            tenant: &'a TenantId,
            ctx: &'a TenantContext<'a>,
        ) -> BoxFuture<'a, Result<Option<Resource>, BoxError>> {
            Box::pin(async move {
                let region = ctx.bean::<Region>().ok_or("no region bean")?;
                Ok(Some(Resource {
                    tenant: format!("{tenant}@{}", region.0),
                    generation: 0,
                }))
            })
        }
    }

    let builder = AppBuilder::new()
        .provide(RegionalSource)
        .plugin(PerTenant::<Resource>::from::<RegionalSource>())
        .provide(Region("eu-west".into()))
        .build_state()
        .await;

    let resource = builder
        .bean_context()
        .get::<Tenanted<Resource>>()
        .get(&tid("acme"))
        .await
        .unwrap();
    assert_eq!(resource.tenant, "acme@eu-west");
}

#[tokio::test]
async fn the_two_plugins_compose_into_a_working_app() {
    // The wiring from the crate docs, end to end.
    let source = ScriptedSource::new().on("ghost", Behaviour::Unknown);
    let router = AppBuilder::new()
        .provide(HeaderTenantResolver::default())
        .provide(source.clone())
        .plugin(Tenancy::resolver::<HeaderTenantResolver>())
        .plugin(PerTenant::<Resource>::from::<ScriptedSource>().max_active(2))
        .build_state()
        .await
        .register_controller::<OrderController>()
        .build();

    let (status, body) = send(router.clone(), "/orders", &[("x-tenant-id", "acme")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "acme");

    let (status, _) = send(router.clone(), "/orders", &[("x-tenant-id", "ghost")]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(router, "/orders", &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(source.creates(), 2, "acme once, ghost once");
}

#[tokio::test]
async fn a_map_provided_by_hand_needs_no_plugin() {
    // The escape hatch the plugin is built on: `Tenanted::new` + `.provide(..)`
    // is a complete wiring for an app that builds its own state.
    let map = Tenanted::new(
        Arc::new(ScriptedSource::new()),
        r2e_core::plugin::GraphHandle::default(),
        r2e_tenant::TenantedSettings::default(),
        None,
    );
    let router = AppBuilder::new()
        .provide(TenantRouter::ready(
            Arc::new(HeaderTenantResolver::default()),
            MissingTenantPolicy::Reject,
            TenantStatuses::default(),
        ))
        .provide(map)
        .build_state()
        .await
        .register_controller::<OrderController>()
        .build();

    let (status, body) = send(router, "/orders", &[("x-tenant-id", "acme")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "acme");
}

#[test]
#[should_panic(expected = "not a way to disable per-tenant resources")]
fn a_max_active_of_zero_is_rejected_on_the_builder() {
    // Nought is a misconfiguration, not an off switch: a cap of zero would open
    // every resource and evict it on the spot. The builder says so where the
    // typo is, rather than at the first request.
    let _ = PerTenant::<Resource>::from::<ScriptedSource>().max_active(0);
}

#[test]
#[should_panic(expected = "tenancy.max-active")]
fn a_max_active_of_zero_is_rejected_in_config() {
    let config = r2e_tenant::TenancyConfig {
        max_active: Some(0),
        ..Default::default()
    };
    let _ = config.max_active();
}

// ── Partial pins: the effects must groom the map the GRAPH exposes ───────────

/// Serve on an ephemeral port until `f` says stop; returns once `run()` did.
///
/// The plugin's sweeper, preload and drain only exist on a real serve path, so
/// these two tests boot a server rather than a router.
async fn serve_while<F, Fut>(app: r2e_core::AppBuilder<impl Clone + Send + Sync + 'static>, f: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let prepared = app.prepare("127.0.0.1:0");
    let stop = prepared.stop_handle();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = tokio::spawn(async move { prepared.run_with_listener(listener).await.is_ok() });
    tokio::time::sleep(Duration::from_millis(50)).await;
    f().await;
    stop.stop();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server did not stop within 5s")
            .expect("server task panicked"),
        "run() returned an error"
    );
}

/// A `Tenanted<Resource>` built by hand, the way a test pins one.
fn hand_built_map(source: &ScriptedSource) -> Tenanted<Resource> {
    Tenanted::new(
        Arc::new(source.clone()),
        r2e_core::plugin::GraphHandle::default(),
        r2e_tenant::TenantedSettings::default(),
        None,
    )
}

#[tokio::test]
async fn the_shutdown_drain_closes_the_pinned_map_not_the_built_one() {
    // Pinning `Tenanted<T>` skips the plugin's projection but still runs its
    // build: requests get the PINNED map while `build` holds another one. An
    // effect that captured its own map would drain an instance nothing can
    // reach and leave every pinned connection open at shutdown.
    //
    // `tenancy.enabled: false` isolates the cleanup lane: the sweeper and the
    // preload are surface effects and are dropped, so the only thing that can
    // dispose here is the shutdown drain.
    let pinned_source = ScriptedSource::new();
    let pinned = hand_built_map(&pinned_source);
    pinned.get(&tid("acme")).await.expect("the pinned map works");

    let app = AppBuilder::new()
        .override_config(config("tenancy:\n  enabled: false\n"))
        .load_config::<()>()
        .override_bean(pinned.clone())
        .provide(ScriptedSource::new())
        .plugin(PerTenant::<Resource>::from::<ScriptedSource>())
        .build_state()
        .await;

    serve_while(app, || async {}).await;

    assert_eq!(
        pinned_source.sorted_disposals(),
        vec!["acme".to_string()],
        "the shutdown drain must close the map the graph exposes (the pinned \
         one), not the invisible map the plugin's build made"
    );
    assert!(
        pinned.peek(&tid("acme")).is_none(),
        "and the pinned map is empty afterwards"
    );
}

#[tokio::test]
async fn the_eager_preload_warms_the_pinned_map_not_the_built_one() {
    // The surface lane of the same rule (sweeper + preload are registered
    // together, from one `after_build` that resolves `Tenanted<T>` once): the
    // warm-up must land in the served map, or the first request pays the
    // creation cost the preload was supposed to have paid.
    let pinned_source = ScriptedSource::new();
    let pinned = hand_built_map(&pinned_source);

    let built_source = ScriptedSource::new();
    let app = AppBuilder::new()
        .override_bean(pinned.clone())
        .provide(built_source.clone())
        .plugin(PerTenant::<Resource>::from::<ScriptedSource>().eager([tid("acme")]))
        .build_state()
        .await;

    let probe = pinned.clone();
    serve_while(app, || async move {
        assert!(
            probe.peek(&tid("acme")).is_some(),
            "the eager preload must warm the pinned map the requests will use"
        );
    })
    .await;

    assert_eq!(
        pinned_source.creates(),
        1,
        "the preload ran against the pinned map's source"
    );
    assert_eq!(
        built_source.creates(),
        0,
        "and never against the invisible map the build constructed"
    );
}
