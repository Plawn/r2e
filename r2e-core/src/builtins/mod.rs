//! Built-in plugins for common cross-cutting concerns.
//!
//! Each one implements the single [`Plugin`](crate::plugin::Plugin) trait and
//! installs with `.plugin(..)` **before** `build_state()`, like every other
//! plugin:
//!
//! ```ignore
//! AppBuilder::new()
//!     .plugin(HttpTrace::new())
//!     .plugin(Cors::permissive())
//!     .plugin(Health)
//!     .build_state()
//!     .await
//!     .serve("0.0.0.0:3000")
//!     .await;
//! ```

pub mod health;
pub mod http_trace;
pub mod request_id;
pub mod secure_headers;

use crate::plugin::{Plugin, PluginBuildContext, PluginBuildError};
use crate::http::layers::cors::CorsLayer;

pub use http_trace::{HttpTrace, HttpTraceBuilder, HttpTraceConfig};

/// CORS plugin.
///
/// Use [`Cors::permissive()`] for a development-friendly configuration that
/// allows any origin, method, and header. Use [`Cors::custom()`] for a
/// production-ready configuration with a specific `CorsLayer`.
pub struct Cors {
    layer: CorsLayer,
}

impl Cors {
    /// Create a permissive CORS plugin (any origin, method, header).
    pub fn permissive() -> Self {
        Self {
            layer: crate::runtime::layers::default_cors(),
        }
    }

    /// Create a CORS plugin with a custom `CorsLayer`.
    pub fn custom(layer: CorsLayer) -> Self {
        Self { layer }
    }
}

impl Plugin for Cors {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        let layer = self.layer;
        ctx.add_layer(move |router| router.layer(layer));
        Ok(())
    }
}

// ── Removed tracing plugins (migration diagnostics) ─────────────────────────

/// **Removed.** The subscriber is installed by `build_state()` from the
/// `tracing:` section; layers are contributed with `.tracing_layer(..)`. Kept
/// only so `.plugin(Tracing)` fails to compile naming
/// `docs/migration/tracing-single-path.md`.
#[doc(hidden)]
#[deprecated(
    note = "the `Tracing` plugin was removed: `build_state()` installs the subscriber from the `tracing:` section — see docs/migration/tracing-single-path.md"
)]
pub struct Tracing;

#[allow(deprecated)]
impl Tracing {
    /// **Removed.** See `docs/migration/tracing-single-path.md`.
    #[doc(hidden)]
    pub fn from_config<'a>(_config: &crate::config::R2eConfig) -> Self
    where
        &'a (): crate::runtime::tracing::TracingPluginRemoved,
    {
        Tracing
    }

    /// **Removed.** See `docs/migration/tracing-single-path.md`.
    #[doc(hidden)]
    pub fn configured<'a>(_config: crate::runtime::tracing_config::TracingConfig) -> Self
    where
        &'a (): crate::runtime::tracing::TracingPluginRemoved,
    {
        Tracing
    }
}

// Not a `Plugin` (its async `build` would capture the gating lifetime): a
// hand-written `PluginInstall` whose unsatisfiable bound makes
// `.plugin(Tracing)` fail with the migration diagnostic. Coherent next to the
// blanket `impl<T: Plugin> PluginInstall for T` because `Tracing` is local and
// implements no `Plugin`.
#[allow(deprecated)]
impl<'a> crate::plugin::PluginInstall for Tracing
where
    &'a (): crate::runtime::tracing::TracingPluginRemoved,
{
    type Provisions = crate::type_list::TNil;
    type Required = crate::type_list::TNil;
    type Controllers = ();

    fn install<P, R, Mods>(
        self,
        _app: crate::builder::AppBuilder<crate::builder::NoState, P, R, Mods>,
    ) -> crate::builder::WithPluginInstalled<Self, P, R, Mods>
    where
        P: crate::type_list::TAppend<Self::Provisions>,
        R: crate::type_list::TAppend<Self::Required>,
        Self::Controllers: crate::di::module::PushPluginCtrls<Self, Mods>,
    {
        unreachable!("`TracingPluginRemoved` is implemented by nothing")
    }
}

/// **Removed.** See [`Tracing`]; `ConfiguredTracing(config)` fails to compile
/// naming `docs/migration/tracing-single-path.md`.
#[doc(hidden)]
#[deprecated(
    note = "the `ConfiguredTracing` plugin was removed: `build_state()` installs the subscriber from the `tracing:` section — see docs/migration/tracing-single-path.md"
)]
pub struct ConfiguredTracing<C: crate::runtime::tracing::TracingPluginRemoved>(pub C);

/// Health-check endpoint plugin.
///
/// # Simple mode
///
/// ```ignore
/// .plugin(Health)  // GET /health → "OK"
/// ```
///
/// # Advanced mode
///
/// ```ignore
/// .plugin(Health::builder()
///     .check(DbHealth::new(pool.clone()))
///     .check(RedisHealth::new(redis.clone()))
///     .build())
/// ```
///
/// Advanced mode provides:
/// - `GET /health` → JSON with aggregated status (200/503)
/// - `GET /health/live` → always 200 (liveness probe)
/// - `GET /health/ready` → 200 if all checks pass, 503 otherwise
///
/// …and a [`HealthRegistry`](crate::builtins::health::HealthRegistry) bean any
/// other plugin can push checks into.
pub struct Health;

impl Health {
    /// Start building an advanced health check configuration.
    pub fn builder() -> crate::builtins::health::HealthBuilder {
        crate::builtins::health::HealthBuilder::new()
    }
}

impl Plugin for Health {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        ctx.after_routes(|routes| {
            routes.register_routes(
                crate::http::Routes::new()
                    .route("/health", crate::http::routing::get(simple_health_handler)),
            );
        });
        Ok(())
    }
}

async fn simple_health_handler(_: (), _: crate::http::Request) -> &'static str {
    "OK"
}

/// Advanced health-check plugin with liveness/readiness probes.
///
/// Created via [`Health::builder()`]. Provides a
/// [`HealthRegistry`](crate::builtins::health::HealthRegistry) bean: any plugin
/// that declares `type Deps = (HealthRegistry,)` can contribute a check from
/// its own `build`, in any install order — the routes are only assembled once
/// the whole graph is built.
pub struct AdvancedHealth {
    checks: Vec<Box<dyn crate::builtins::health::HealthIndicatorErased>>,
    cache_ttl: Option<std::time::Duration>,
}

impl AdvancedHealth {
    pub(crate) fn new(
        checks: Vec<Box<dyn crate::builtins::health::HealthIndicatorErased>>,
        cache_ttl: Option<std::time::Duration>,
    ) -> Self {
        Self { checks, cache_ttl }
    }
}

impl Plugin for AdvancedHealth {
    type Provided = (crate::builtins::health::HealthRegistry,);
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        let registry = crate::builtins::health::HealthRegistry::new();
        for check in self.checks {
            registry.register_boxed(check);
        }
        if let Some(ttl) = self.cache_ttl {
            registry.set_cache_ttl(ttl);
        }

        // Mount in the Routes stage, i.e. after every plugin `build` has had
        // the chance to push a check into the registry.
        let for_routes = registry.clone();
        ctx.after_routes(move |routes| {
            use std::sync::Arc;
            let state = Arc::new(for_routes.into_state());
            routes.register_routes(
                crate::http::Routes::<Arc<crate::builtins::health::HealthState>>::new()
                    .route(
                        "/health",
                        crate::http::routing::get(crate::builtins::health::health_handler),
                    )
                    .route(
                        "/health/live",
                        crate::http::routing::get(crate::builtins::health::liveness_handler),
                    )
                    .route(
                        "/health/ready",
                        crate::http::routing::get(crate::builtins::health::readiness_handler),
                    )
                    .with_state(state),
            );
        });

        Ok((registry,))
    }
}

/// Dev-mode reload endpoints plugin.
///
/// Adds `/__r2e_dev/status` and `/__r2e_dev/ping` endpoints for
/// tooling and browser scripts to detect server restarts.
///
/// Also adds a `Cache-Control: no-store` layer to prevent browsers
/// from caching API responses during development (which would cause
/// stale values in Swagger UI after hot-reload).
///
/// Installing it explicitly is idempotent with the automatic install the
/// `dev-reload` feature performs at `prepare()`.
pub struct DevReload;

impl Plugin for DevReload {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        ctx.after_build(|dctx| {
            if !dctx.mark_dev_reload_applied() {
                return;
            }
            dctx.add_layer(Box::new(|routes: crate::http::Routes| {
                routes.middleware_fn(crate::runtime::dev::dev_headers_middleware)
            }));
            dctx.after_routes(|routes| {
                routes.register_routes(crate::runtime::dev::dev_routes());
            });
        });
        Ok(())
    }
}

/// Trailing-slash normalization plugin.
///
/// Removes trailing slashes from request paths, so `/users/` becomes `/users`.
/// This ensures consistent routing regardless of whether clients include
/// a trailing slash.
///
/// Implemented as a pre-routing URI rewrite (tower-http `NormalizePath`)
/// wrapping the whole router: the slash is stripped before routing, so the
/// request is routed exactly once and carries `MatchedPath` through all
/// instrumentation layers (metrics, tracing). It can be installed at any
/// point in the plugin chain.
///
/// Notes on the exact rewrite semantics (tower-http `trim_trailing_slash`):
///
/// - Routes declared with a literal trailing slash (e.g. `#[get("/foo/")]`)
///   are unreachable — the incoming path is always trimmed first.
/// - A leading run of slashes is also collapsed: `//admin` is rewritten to
///   `/admin` and routed there (without the plugin it would be a 404). This
///   matters for raw-path consumers such as `#[fallback]` gateway proxies,
///   which see and forward the normalized path.
pub struct NormalizePath;

impl Plugin for NormalizePath {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        ctx.after_build(|dctx| dctx.enable_normalize_path());
        Ok(())
    }
}
