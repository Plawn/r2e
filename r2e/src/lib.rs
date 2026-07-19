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
//! | `openfga`     | no      | `r2e-openfga`             |
//! | `events-kafka`    | no  | `r2e-events-kafka` (Apache Kafka backend) |
//! | `events-pulsar`   | no  | `r2e-events-pulsar` (Apache Pulsar backend) |
//! | `events-rabbitmq` | no  | `r2e-events-rabbitmq` (RabbitMQ/AMQP backend) |
//! | `static`      | no      | `r2e-static` (embedded static file serving + SPA fallback) |
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

#[cfg(feature = "static")]
pub use r2e_static;

#[cfg(feature = "observability")]
pub use r2e_observability;

#[cfg(feature = "dev-reload")]
pub mod devtools {
    pub use r2e_core::dev::mark_hot_reload_loop;
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
#[macro_export]
macro_rules! app_main {
    ($app:ty) => {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/app.rs"));

        #[$crate::main]
        async fn main() {
            $crate::launch!($app).await.unwrap();
        }
    };
}

/// Launch an [`App`](r2e_core::App) from a custom `main`.
///
/// ```ignore
/// #[r2e::main]
/// async fn main() {
///     r2e::launch!(MyApp).await.unwrap();
/// }
/// ```
///
/// Expands to an `async` block that yields the same
/// `Result<(), Box<dyn std::error::Error>>` as [`launch`](r2e_core::launch), so
/// it is awaited exactly like the function form.
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
#[macro_export]
macro_rules! launch {
    ($app:ty) => {
        async {
            #[cfg(not(feature = "dev-reload"))]
            {
                $crate::launch::<$app>().await
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
                    let __app =
                        <$app as $crate::App>::build($crate::AppBuilder::new(), __env).await;
                    if let ::core::result::Result::Err(__e) =
                        $crate::BootableApp::serve_auto(__app).await
                    {
                        ::std::eprintln!("[r2e dev-reload] serve failed: {}", __e);
                    }
                }

                let __env = <$app as $crate::App>::setup().await;
                // Enable the process-global dev-reload caches (bean-graph
                // fingerprinting, instance reuse, lifecycle skip): they must
                // engage only under the actual hot-patch loop, never in a
                // process that merely compiled the feature.
                $crate::devtools::mark_hot_reload_loop();
                $crate::devtools::serve_with_hotreload_env(__env, |__e| __r2e_server(__e)).await;
                ::core::result::Result::<(), ::std::boxed::Box<dyn ::std::error::Error>>::Ok(())
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

    #[cfg(feature = "openapi")]
    pub use r2e_openapi::schemars::JsonSchema;
}
