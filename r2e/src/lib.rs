//! R2E — a Quarkus-like ergonomic layer over Axum.
//!
//! This facade crate re-exports all R2E sub-crates through a single
//! dependency with feature flags. Import everything you need with:
//!
//! ```ignore
//! use r2e::prelude::*;
//! ```
//!
//! # Feature flags
//!
//! | Feature       | Default | Crate                     |
//! |---------------|---------|---------------------------|
//! | `security`    | **yes** | `r2e-security`            |
//! | `events`      | **yes** | `r2e-events`              |
//! | `utils`       | **yes** | `r2e-utils`               |
//! | `data-sqlx`   | no      | `r2e-data-sqlx`           |
//! | `data-diesel` | no      | `r2e-data-diesel`         |
//! | `sqlx-sqlite` / `sqlx-postgres` / `sqlx-mysql` | no | managed SQLx transactions |
//! | `diesel-sqlite` / `diesel-postgres` / `diesel-mysql` | no | managed Diesel transactions |
//! | `scheduler`   | no      | `r2e-scheduler`           |
//! | `executor`    | no      | `r2e-executor` (managed task pool, à la J2EE `ManagedExecutorService`) |
//! | `cache`       | no      | `r2e-cache`               |
//! | `rate-limit`  | no      | `r2e-rate-limit`          |
//! | `openapi`     | no      | `r2e-openapi` (also add `schemars = "1"` to your deps) |
//! | `prometheus`  | no      | `r2e-prometheus`          |
//! | `metrics-facade` | no   | `r2e-prometheus/metrics-facade` — HTTP metrics through the `metrics` facade into the app's own recorder (**not** in `full`) |
//! | `openfga`     | no      | `r2e-openfga`             |
//! | `events-kafka`    | no  | `r2e-events-kafka` (Apache Kafka backend) |
//! | `events-pulsar`   | no  | `r2e-events-pulsar` (Apache Pulsar backend) |
//! | `events-rabbitmq` | no  | `r2e-events-rabbitmq` (RabbitMQ/AMQP backend) |
//! | `static`      | no      | `r2e-static` (embedded static file serving + SPA fallback) |
//! | `tenant`      | no      | `r2e-tenant` (multi-tenant bean routing: `Tenant<T>`, per-tenant resources) |
//! | `tenant-sqlx` | no      | per-tenant SQLx pools/transactions (`TenantPools`, `TenantTx`, `PoolSource`) |
//! | `tenant-diesel` | no    | per-tenant Diesel r2d2 pools/transactions (`TenantPools`, `TenantTx`, `PoolSource`) |
//! | `validation`  | no      | `r2e-core/validation`     |
//! | `dev-reload`  | no      | `r2e-devtools` (Subsecond hot-patch, **not** in `full`) |
//! | `full`        | no      | Bundled framework modules; database/event backends, QUIC, and dev reload stay opt-in |

// Re-export sub-crates as public modules so they're accessible as
// `r2e::r2e_core`, `r2e::r2e_events`, etc.
//
// The proc macros use `proc-macro-crate` to detect whether the user depends
// on `r2e` (facade) or individual crates, and generate the correct paths.
pub extern crate r2e_core;
pub extern crate r2e_macros;

#[cfg(feature = "rate-limit")]
pub extern crate r2e_rate_limit;

// Re-export everything from r2e-core at the top level for convenience.
pub use r2e_core::*;

#[cfg(feature = "security")]
pub use r2e_security;

#[cfg(feature = "events")]
pub use r2e_events;

#[cfg(feature = "events-iggy")]
pub use r2e_events_iggy;

#[cfg(feature = "events-kafka")]
pub use r2e_events_kafka;

#[cfg(feature = "events-pulsar")]
pub use r2e_events_pulsar;

#[cfg(feature = "events-rabbitmq")]
pub use r2e_events_rabbitmq;

#[cfg(feature = "utils")]
pub use r2e_utils;

#[cfg(feature = "data-sqlx")]
pub use r2e_data_sqlx;

#[cfg(feature = "data-diesel")]
pub use r2e_data_diesel;

#[cfg(feature = "scheduler")]
pub use r2e_scheduler;

#[cfg(feature = "executor")]
pub use r2e_executor;

#[cfg(feature = "cache")]
pub use r2e_cache;

#[cfg(feature = "oidc")]
pub use r2e_oidc;

#[cfg(feature = "openapi")]
pub use r2e_openapi;

#[cfg(feature = "prometheus")]
pub use r2e_prometheus;

#[cfg(feature = "openfga")]
pub use r2e_openfga;

#[cfg(feature = "grpc")]
pub use r2e_grpc;

#[cfg(feature = "mcp")]
pub use r2e_mcp;

#[cfg(feature = "static")]
pub use r2e_static;

#[cfg(feature = "tenant")]
pub use r2e_tenant;

/// Multi-tenant bean routing: [`Tenant<T>`](r2e_tenant::Tenant) extractors,
/// resolvers, per-tenant resource maps, and the two plugins.
///
/// The readable alias for `r2e::r2e_tenant` — `use r2e::tenant::{PerTenant,
/// Tenancy, Tenant};`.
#[cfg(feature = "tenant")]
pub mod tenant {
    pub use r2e_tenant::*;
}

#[cfg(feature = "observability")]
pub use r2e_observability;

#[cfg(feature = "dev-reload")]
pub mod devtools {
    pub use r2e_core::runtime::dev::mark_hot_reload_loop;
    pub use r2e_devtools::*;
}

/// Declare the standard binary entry point for an [`App`](r2e_core::App).
///
/// The macro includes the package's canonical `src/app.rs` directly in the
/// binary tip crate, generates `main`, and delegates to [`launch!`]. The same
/// `app.rs` can therefore be included by `lib.rs` for integration tests without
/// making users maintain `cfg` or crate-name-dependent imports in `main.rs`.
///
/// ```ignore
/// r2e::app_main!(MyApp);
/// ```
///
/// This conventional form expects the application source at `src/app.rs`. Use
/// `#[r2e::main]` with [`launch!`] directly when a custom entry point is needed.
///
/// # Tracing
///
/// By default the global `tracing` subscriber is installed **after**
/// [`App::setup`](r2e_core::App::setup) returns, so an app that builds its own
/// subscriber in `setup` wins (`init_tracing` is idempotent). Opt out entirely
/// with the same spelling `#[r2e::main]` uses:
///
/// ```ignore
/// r2e::app_main!(MyApp, tracing = false);
/// ```
///
/// R2E then installs nothing, leaving the subscriber to the app — including to
/// a `Tracing::from_config(..)` / `ConfiguredTracing` plugin installed in
/// `build`, which would otherwise lose the race to the entry point.
#[macro_export]
macro_rules! app_main {
    ($app:ty) => {
        $crate::app_main!($app, tracing = true);
    };
    ($app:ty, tracing = $tracing:expr) => {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/app.rs"));

        // `tracing = false` on the attribute always: the subscriber is
        // installed by `launch!` *after* `App::setup`, not before the runtime
        // starts, so the app has the first word on it.
        #[$crate::main(tracing = false)]
        async fn main() {
            // A boot failure is an operational error, not a bug: print one
            // line (plus the `source()` chain) and exit non-zero, rather than
            // the panic + backtrace + exit code 101 that `unwrap` would give.
            $crate::exit_on_boot_error($crate::launch!($app, tracing = $tracing).await);
        }
    };
}

/// Launch an [`App`](r2e_core::App) from a custom `main`.
///
/// ```ignore
/// #[r2e::main]
/// async fn main() {
///     // Same contract as `app_main!`: one `error:` line and exit code 1.
///     r2e::exit_on_boot_error(r2e::launch!(MyApp).await);
/// }
/// ```
///
/// Expands to an `async` block that yields the same
/// `Result<(), BootError>` as [`launch`](r2e_core::launch), so it is awaited
/// exactly like the function form.
///
/// # Why this is a macro and not just `launch::<A>()`
///
/// Under the `dev-reload` feature this macro drives the Subsecond hot-patch
/// loop, and it must do so from a **concrete, named function defined in the tip
/// crate** (the crate that owns `main.rs`). Subsecond only remaps function
/// symbols it attributes to the tip crate; a generic dispatcher monomorphised
/// from `r2e-core` is *not* remapped — its jump-table lookup misses and
/// hot-patches never reach the rebuilt `App::build`. Because a `macro_rules!`
/// expands at the call site, the `__r2e_server` function it emits lives in the
/// user's crate, so patches apply. Without `dev-reload` the macro simply calls
/// [`launch::<A>()`](r2e_core::launch).
///
/// [`app_main!`] compiles the canonical `src/app.rs` source directly in the
/// binary. That keeps `App::build`, controllers, and services in the tip crate
/// while `lib.rs` includes the same source for integration tests.
///
/// `App::setup` runs **once** (its `Env` survives hot-patches); `App::build`
/// and serve re-run on every patch, and `build`'s `load_config` re-reads
/// `application.yaml` per patch so config edits apply on the next patch.
///
/// # Tracing
///
/// The global subscriber is installed **after** `App::setup` (and, under
/// `dev-reload`, once — outside the patch loop). `launch!(MyApp, tracing =
/// false)` skips it entirely, leaving the subscriber to the app. Calling
/// `launch!` from a `#[r2e::main]` that already initialised tracing is
/// harmless: `init_tracing` is idempotent.
#[macro_export]
macro_rules! launch {
    ($app:ty) => {
        $crate::launch!($app, tracing = true)
    };
    ($app:ty, tracing = $tracing:expr) => {
        async {
            #[cfg(not(feature = "dev-reload"))]
            {
                let mut __opts = $crate::LaunchOptions::default();
                __opts.tracing = $tracing;
                $crate::launch_with::<$app>(__opts).await
            }
            #[cfg(feature = "dev-reload")]
            {
                // Concrete, named function expanded into the *tip* crate.
                // Subsecond can discover and remap it, so each hot-patch
                // re-runs the rebuilt `App::build`. The closure handed to the
                // loop stays non-capturing (a ZST) so `HotFn` dispatches
                // through the jump table.
                async fn __r2e_server(__env: <$app as $crate::App>::Env) {
                    ::std::eprintln!("[r2e dev-reload] (re)building app");
                    // A failing `build` must not kill the loop: report it and
                    // wait for the next hot-patch to fix it.
                    let __app = match <$app as $crate::App>::build($crate::AppBuilder::new(), __env)
                        .await
                    {
                        ::core::result::Result::Ok(__a) => {
                            // The cycle assembled: promote the graph this
                            // cycle staged into the dev-reload caches, so the
                            // next patch may reuse it.
                            $crate::commit_dev_cycle();
                            __a
                        }
                        ::core::result::Result::Err(__e) => {
                            // A failed cycle must leave nothing behind: drop
                            // the staged graph (and the beans it built) and
                            // keep the last successful cycle's caches, so the
                            // next patch neither reuses a broken graph nor
                            // skips its startup lifecycle.
                            $crate::rollback_dev_cycle();
                            ::std::eprintln!("[r2e dev-reload] build failed: {}", __e);
                            return;
                        }
                    };
                    if let ::core::result::Result::Err(__e) =
                        $crate::BootableApp::serve_auto(__app).await
                    {
                        ::std::eprintln!("[r2e dev-reload] serve failed: {}", __e);
                    }
                }

                // `setup` runs once, before the loop — a failure there is
                // fatal and propagates out of `launch!` like the non-dev arm.
                let __env = match <$app as $crate::App>::setup().await {
                    ::core::result::Result::Ok(__e) => __e,
                    ::core::result::Result::Err(__e) => return ::core::result::Result::Err(__e),
                };
                // Same ordering as the non-dev arm: the subscriber goes in
                // after `setup`, once, outside the hot-patch loop (it is a
                // process-global, one-shot install).
                if $tracing {
                    $crate::init_tracing();
                }
                // Enable the process-global dev-reload caches (bean-graph
                // fingerprinting, instance reuse, lifecycle skip): they must
                // engage only under the actual hot-patch loop, never in a
                // process that merely compiled the feature.
                $crate::devtools::mark_hot_reload_loop();
                $crate::devtools::serve_with_hotreload_env(__env, |__e| __r2e_server(__e)).await;
                ::core::result::Result::<(), $crate::BootError>::Ok(())
            }
        }
    };
}

/// Convenience type aliases that depend on types from optional sub-crates.
pub mod types {
    pub use r2e_core::types::*;

    /// Paginated JSON result — `Result<Json<Page<T>>, HttpError>`.
    ///
    /// ```ignore
    /// #[get("/users")]
    /// async fn list(&self, pageable: Pageable) -> PagedResult<User> {
    ///     Ok(Json(self.service.list(pageable).await?))
    /// }
    /// ```
    pub type PagedResult<T> = Result<r2e_core::http::Json<r2e_core::Page<T>>, r2e_core::HttpError>;
}

/// Unified prelude — import everything with `use r2e::prelude::*`.
///
/// Includes the core prelude plus types from all enabled feature crates.
pub mod prelude {
    pub use crate::types::*;
    pub use r2e_core::prelude::*;

    #[cfg(feature = "security")]
    pub use r2e_security::prelude::*;

    #[cfg(feature = "data-sqlx")]
    pub use r2e_data_sqlx::prelude::*;

    #[cfg(feature = "data-diesel")]
    pub use r2e_data_diesel::prelude::*;

    #[cfg(feature = "events")]
    pub use r2e_events::prelude::*;

    #[cfg(feature = "scheduler")]
    pub use r2e_scheduler::prelude::*;

    #[cfg(feature = "events-iggy")]
    pub use r2e_events_iggy::prelude::*;

    #[cfg(feature = "events-kafka")]
    pub use r2e_events_kafka::prelude::*;

    #[cfg(feature = "events-pulsar")]
    pub use r2e_events_pulsar::prelude::*;

    #[cfg(feature = "events-rabbitmq")]
    pub use r2e_events_rabbitmq::prelude::*;

    #[cfg(feature = "utils")]
    pub use r2e_utils::prelude::*;

    #[cfg(feature = "oidc")]
    pub use r2e_oidc::prelude::*;

    #[cfg(feature = "openfga")]
    pub use r2e_openfga::prelude::*;

    #[cfg(feature = "grpc")]
    pub use r2e_grpc::prelude::*;

    #[cfg(feature = "mcp")]
    pub use r2e_mcp::prelude::*;

    #[cfg(feature = "openapi")]
    pub use r2e_openapi::schemars::JsonSchema;
}
