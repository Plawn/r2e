//! [`BootableApp`]: the return contract of [`App::build`](crate::App::build).
//!
//! [`App`](crate::App) is the app's single declaration, shared between
//! production and tests:
//!
//! ```ignore
//! // lib.rs
//! impl App for MyApp {
//!     type Env = ();
//!     async fn setup() -> Result<(), BootError> { Ok(()) }
//!     async fn build(b: AppBuilder, _env: ()) -> Result<impl BootableApp, BootError> {
//!         Ok(b.load_config::<AppConfig>()
//!             .register::<UserService>()
//!             .plugin(Health)
//!             .try_build_state().await?
//!             .register_controllers::<(UserController,)>())
//!     }
//! }
//!
//! // main.rs — `app_main!` generates the equivalent: a boot failure prints
//! // the error chain and exits non-zero instead of panicking.
//! r2e::app_main!(MyApp);
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
    /// beans, EventBus bridges) plus the controller `#[post_construct]` and
    /// `#[on_start]` hooks. The in-process test entry point — used by
    /// `TestApp::boot` so startup behaviour matches production in tests.
    ///
    /// Fallible: a startup hook that returns `Err` aborts the boot here, which
    /// is what lets `TestApp::try_boot*` return it instead of panicking
    /// underneath the harness.
    fn into_router_with_consumers(
        self,
    ) -> impl Future<Output = Result<crate::http::Router, crate::beans::BootError>>;

    /// Build and serve on an explicit address.
    fn serve(self, addr: &str) -> impl Future<Output = Result<(), crate::beans::BootError>>;

    /// Build and serve, reading `server.host`/`server.port` from config
    /// (production entry point).
    fn serve_auto(self) -> impl Future<Output = Result<(), crate::beans::BootError>>;
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

    fn into_router_with_consumers(
        self,
    ) -> impl Future<Output = Result<crate::http::Router, crate::beans::BootError>> {
        self.try_build_with_consumers()
    }

    fn serve(self, addr: &str) -> impl Future<Output = Result<(), crate::beans::BootError>> {
        AppBuilder::serve(self, addr)
    }

    fn serve_auto(self) -> impl Future<Output = Result<(), crate::beans::BootError>> {
        AppBuilder::serve_auto(self)
    }
}
