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

use crate::plugin::{Plugin, PluginBuildContext, PluginBuildError, PluginSetupContext};
use tower_http::cors::CorsLayer;

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

/// Log **subscriber** plugin — installs the global `tracing` subscriber with
/// R2E's built-in defaults.
///
/// It contributes **no HTTP layer**: per-request spans and the request summary
/// line are [`HttpTrace`]'s job, and the two are installed independently.
///
/// The subscriber is a process-global, one-shot install, done from `setup`
/// (before any bean is built, so boot logs are captured).
///
/// # Do I need it?
///
/// | Entry point | Subscriber comes from |
/// |---|---|
/// | `r2e::launch` / `#[r2e::main]` / `app_main!` / `#[r2e::test]` | the entry point, from the app's own `tracing:` section — **do not** add this plugin |
/// | a hand-written `main` / an embedded `AppBuilder` | `.plugin(Tracing)` (built-in defaults) or [`Tracing::from_config`] (the `tracing:` section) |
/// | `r2e_observability::Observability` | that plugin (fmt + `tracing-opentelemetry`) |
///
/// # Who owns what
///
/// | Plugin | Owns |
/// |---|---|
/// | `Tracing` / [`ConfiguredTracing`] | the subscriber: format, filter, ansi, span events |
/// | [`HttpTrace`] | the per-request span, summary event, request id, exclusions |
/// | `r2e_observability::Observability` | subscriber **+** OTLP export/propagation; installs the `HttpTrace` layer with an OpenTelemetry span shape |
///
/// `Observability` supersedes `Tracing` — do not install both.
///
/// [`init_tracing()`]: crate::init_tracing
///
/// # Example
///
/// ```ignore
/// AppBuilder::new()
///     .plugin(Tracing)              // subscriber
///     .plugin(HttpTrace::new())     // one span + one summary line per request
///     .build_state()
///     .await
///     .serve("0.0.0.0:3000")
///     .await;
/// ```
pub struct Tracing;

impl Tracing {
    /// Create a tracing plugin configured from a [`TracingConfig`].
    ///
    /// [`TracingConfig`]: crate::runtime::tracing_config::TracingConfig
    pub fn configured(config: crate::runtime::tracing_config::TracingConfig) -> ConfiguredTracing {
        ConfiguredTracing(config)
    }

    /// Create a tracing plugin configured from [`R2eConfig`], reading
    /// keys under the `tracing` prefix.
    ///
    /// [`R2eConfig`]: crate::config::R2eConfig
    pub fn from_config(r2e_config: &crate::config::R2eConfig) -> ConfiguredTracing {
        use crate::config::ConfigProperties;
        let config =
            crate::runtime::tracing_config::TracingConfig::from_config(r2e_config, Some("tracing"))
                .unwrap_or_default();
        ConfiguredTracing(config)
    }
}

impl Plugin for Tracing {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    // The subscriber is a process-global, one-shot install: doing it in
    // `setup` (which runs at `.plugin()` time, before the graph is built)
    // means bean construction and every other plugin's `build` are already
    // instrumented.
    fn setup(&mut self, _ctx: &mut PluginSetupContext) {
        crate::runtime::layers::init_tracing();
    }

    // The subscriber is the whole plugin: no router effect at all. Per-request
    // spans, the summary line and request ids come from [`HttpTrace`].
    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        Ok(())
    }
}

/// Subscriber plugin with explicit configuration.
///
/// Created via [`Tracing::configured()`] or [`Tracing::from_config()`]. Like
/// [`Tracing`] it installs **only** the subscriber — pair it with
/// [`HttpTrace`] for per-request spans.
pub struct ConfiguredTracing(pub crate::runtime::tracing_config::TracingConfig);

impl Plugin for ConfiguredTracing {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    // An explicit configuration that loses the race to an earlier subscriber
    // is not honoured at all, so it says so — unless the winner already logs
    // exactly what this one asked for, which is what happens when the entry
    // point read the same `tracing:` section a moment earlier.
    fn setup(&mut self, _ctx: &mut PluginSetupContext) {
        if let Err(lost) = crate::runtime::layers::try_init_tracing_with_config(&self.0) {
            crate::runtime::layers::warn_if_output_differs(&lost, &self.0);
        }
    }

    // No router effect — see [`Tracing`].
    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        Ok(())
    }
}

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
                crate::http::Router::new()
                    .route("/health", crate::http::routing::get(simple_health_handler)),
            );
        });
        Ok(())
    }
}

async fn simple_health_handler() -> &'static str {
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
            let s1 = Arc::clone(&state);
            routes.register_routes(
                crate::http::Router::new()
                    .route(
                        "/health",
                        crate::http::routing::get(crate::builtins::health::health_handler)
                            .with_state(state),
                    )
                    .route(
                        "/health/live",
                        crate::http::routing::get(crate::builtins::health::liveness_handler),
                    )
                    .route(
                        "/health/ready",
                        crate::http::routing::get(crate::builtins::health::readiness_handler)
                            .with_state(s1),
                    ),
            );
        });

        Ok((registry,))
    }
}

/// Error-handling plugin.
///
/// Adds a catch-panic layer that converts panics into JSON 500 responses.
///
/// **Rarely needed.** R2E already installs that layer twice on its own — once
/// *innermost*, below every `add_layer`, so a handler panic is recorded as a
/// 500 by the instrumentation instead of unwinding through it, and once
/// outermost as a last-resort net (see [`crate::runtime::panic`]). This plugin
/// adds a third copy at its own install slot, which only changes anything for
/// a panic raised by a layer installed *after* it.
pub struct ErrorHandling;

impl Plugin for ErrorHandling {
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
        ctx.add_layer(|router| router.layer(crate::runtime::layers::catch_panic_layer()));
        Ok(())
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
            dctx.add_layer(Box::new(|router: crate::http::Router| {
                router.layer(crate::http::middleware::from_fn(
                    crate::runtime::dev::dev_headers_middleware,
                ))
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
