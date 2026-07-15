//! [`BootableApp`]: the return contract of [`App::build`](crate::App::build).
//!
//! [`App`](crate::App) is the app's single declaration, shared between
//! production and tests:
//!
//! ```ignore
//! // lib.rs
//! impl App for MyApp {
//!     type Env = ();
//!     async fn setup() {}
//!     async fn build(b: AppBuilder, _env: ()) -> impl BootableApp {
//!         b.load_config::<AppConfig>()
//!             .register::<UserService>()
//!             .build_state().await
//!             .with(Health)
//!             .register_controllers::<(UserController,)>()
//!     }
//! }
//!
//! // main.rs
//! #[r2e::main]
//! async fn main() {
//!     r2e::launch::<MyApp>().await.unwrap();
//! }
//!
//! // tests — the harness pre-configures the builder (profile, pinned mocks,
//! // config overrides) before running the same App:
//! let app = TestApp::boot::<MyApp>().await;
//! ```
//!
//! The inferred HList state type cannot be named by user code, so `build`
//! returns `impl BootableApp`; the trait exposes exactly what the consumers
//! ([`launch`](crate::launch), the test harness) need.

use std::future::Future;

use super::*;

/// Assembled application, ready to serve or to be dissected by a test
/// harness. Implemented by the typed [`AppBuilder<T>`].
pub trait BootableApp: Sized {
    /// The resolved bean graph (test harnesses read beans out of it by type).
    fn bean_context(&self) -> Arc<crate::beans::BeanContext>;

    /// The loaded [`R2eConfig`](crate::config::R2eConfig), if any.
    fn r2e_config(&self) -> Option<crate::config::R2eConfig>;

    /// Assemble the final router without starting event consumers.
    fn into_router(self) -> crate::http::Router;

    /// Assemble the final router and run the consumer registrations that
    /// `serve()` would run at startup (`#[consumer]` methods, subscriber
    /// beans, EventBus bridges). The in-process test entry point — used by
    /// `TestApp::boot` so event consumers have production parity in tests.
    fn into_router_with_consumers(self) -> impl Future<Output = crate::http::Router>;

    /// Build and serve on an explicit address.
    fn serve(self, addr: &str) -> impl Future<Output = Result<(), Box<dyn std::error::Error>>>;

    /// Build and serve, reading `server.host`/`server.port` from config
    /// (production entry point).
    fn serve_auto(self) -> impl Future<Output = Result<(), Box<dyn std::error::Error>>>;
}

impl<T: Clone + Send + Sync + 'static> BootableApp for AppBuilder<T> {
    fn bean_context(&self) -> Arc<crate::beans::BeanContext> {
        Arc::clone(&self.bean_context)
    }

    fn r2e_config(&self) -> Option<crate::config::R2eConfig> {
        self.shared.config.clone()
    }

    fn into_router(self) -> crate::http::Router {
        self.build()
    }

    fn into_router_with_consumers(self) -> impl Future<Output = crate::http::Router> {
        self.build_with_consumers()
    }

    fn serve(self, addr: &str) -> impl Future<Output = Result<(), Box<dyn std::error::Error>>> {
        AppBuilder::serve(self, addr)
    }

    fn serve_auto(self) -> impl Future<Output = Result<(), Box<dyn std::error::Error>>> {
        AppBuilder::serve_auto(self)
    }
}
