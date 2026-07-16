// Canonical example application source.
//
// `lib.rs` includes this file for integration tests. `app_main!` includes the
// same file directly in the binary tip crate so Subsecond patches controllers
// and services for real.

use std::time::Duration;

use r2e::prelude::*;
use r2e::r2e_observability::{Observability, ObservabilityConfig};
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};
use r2e::r2e_prometheus::Prometheus;
use r2e::r2e_scheduler::Scheduler;

pub mod controllers;
pub mod db_identity;
pub mod env;
pub mod error;
pub mod models;
pub mod services;

use controllers::account_controller::AccountController;
use controllers::config_controller::ConfigController;
use controllers::data_controller::DataController;
use controllers::db_identity_controller::IdentityController;
use controllers::event_controller::UserEventConsumer;
use controllers::mixed_controller::MixedController;
use controllers::notification_controller::NotificationController;
use controllers::proxy_controller::ProxyController;
use controllers::report_controller::ReportController;
use controllers::scheduled_controller::ScheduledJobs;
use controllers::sse_controller::SseController;
use controllers::upload_controller::UploadController;
use controllers::user_controller::UserController;
use controllers::ws_controller::WsEchoController;
pub use env::demo_token;
use env::{provision_env, AppEnv};
use services::UserService;

/// The "users" vertical slice as a feature module: one `register_module`
/// call registers the service and both controllers. `UserService` is
/// exported (other controllers inject it); the imports are satisfied by the
/// app's `.provide`/`.load_config` calls below. Decorator deps count too:
/// `UserController`'s rate-limit guard and cache interceptor read
/// `RateLimitRegistry` and the cache store bean, so the module imports them.
#[module(
    providers(UserService),
    controllers(UserController, UserEventConsumer),
    exports(UserService),
    imports(
        LocalEventBus,
        sqlx::SqlitePool,
        R2eConfig,
        r2e::r2e_rate_limit::RateLimitRegistry,
        std::sync::Arc<dyn r2e::r2e_cache::CacheStore>,
    )
)]
pub struct UserModule;

/// The canonical application blueprint. Production (`r2e::app_main!(ExampleApp)`),
/// dev hot-reload, and tests (`#[r2e::test(app = example_app::ExampleApp)]` /
/// `TestApp::boot::<ExampleApp>()`) all go through this single [`App`] impl.
pub struct ExampleApp;

impl App for ExampleApp {
    /// Long-lived resources; in dev mode they survive hot-patches.
    type Env = AppEnv;

    async fn setup() -> AppEnv {
        // Print a demo JWT for curl usage (harmless in tests; this is a demo app).
        println!("=== Test JWT (valid 1h) ===");
        println!("{}", demo_token());
        println!();

        provision_env().await
    }

    async fn build(b: AppBuilder, env: AppEnv) -> impl BootableApp {
        b.plugin(Scheduler)
            .plugin(
                Prometheus::builder()
                    .endpoint("/metrics")
                    .namespace("r2e")
                    .exclude_path("/health")
                    .exclude_path("/metrics")
                    .build(),
            )
            .load_config::<controllers::config_controller::RootConfig>()
            .provide(env.event_bus)
            .provide(env.pool)
            .provide(env.claims_validator)
            .provide(r2e::r2e_rate_limit::RateLimitRegistry::default())
            .provide(r2e::r2e_cache::InMemoryStore::shared())
            .provide(env.sse_broadcaster)
            .provide(SseTopic::<models::UserCreatedEvent>::new(64).with_event_name("user_created"))
            .provide(env.notification_service)
            .register_module::<UserModule>()
            .build_state()
            .await
            // EventBus↔SSE bridge: every UserCreatedEvent emitted on the bus is
            // broadcast on the topic (served at /sse/users) with no liaison code.
            .bridge_sse::<LocalEventBus, models::UserCreatedEvent>()
            // Dynamic scheduled task: the schedule is parsed from a string at
            // runtime (config-driven), unlike the #[scheduled] methods on
            // ScheduledJobs. Both show up in the ScheduledJobRegistry bean.
            .schedule_task(ScheduledTaskDef::from_fn(
                "dynamic_heartbeat",
                "30s".parse().expect("valid schedule"),
                || async { tracing::debug!("dynamic heartbeat tick") },
            ))
            .with(Health)
            .with(RequestIdPlugin)
            .with(SecureHeaders::default())
            .with(Cors::permissive())
            .with(Observability::new(
                ObservabilityConfig::new("r2e-example")
                    .with_service_version("0.1.0")
                    .capture_header("x-request-id"),
            ))
            .with(ErrorHandling)
            .with_layer(tower_http::timeout::TimeoutLayer::with_status_code(
                r2e::http::StatusCode::REQUEST_TIMEOUT,
                Duration::from_secs(30),
            ))
            .with(OpenApiPlugin::new(
                OpenApiConfig::new("R2E Example API", "0.1.1")
                    .with_description("Demo application showcasing all R2E features")
                    .with_docs_ui(true),
            ))
            .on_start(|_state| async move {
                tracing::info!("R2E example-app startup hook executed");
                Ok(())
            })
            .on_stop(|_| async {
                tracing::info!("R2E example-app shutdown hook executed");
            })
            .register_controllers::<(
                AccountController,
                ConfigController,
                DataController,
                MixedController,
                IdentityController,
                ScheduledJobs,
                SseController,
                WsEchoController,
                NotificationController,
                ReportController,
                UploadController,
                ProxyController,
            )>()
            .with(NormalizePath)
    }
}
