//! [`BootableApp`]: the return contract of application **blueprint**
//! functions.
//!
//! A blueprint is the app's single assembly function, shared between
//! production and tests:
//!
//! ```ignore
//! // lib.rs
//! pub async fn app(b: AppBuilder) -> impl BootableApp {
//!     b.load_config::<AppConfig>()
//!         .register::<UserService>()
//!         .build_state().await
//!         .with(Health)
//!         .register_controllers::<(UserController,)>()
//! }
//!
//! // main.rs
//! #[r2e::main]
//! async fn main() {
//!     example_app::app(AppBuilder::new()).await.serve_auto().await.unwrap();
//! }
//!
//! // tests — the harness pre-configures the builder (profile, pinned mocks,
//! // config overrides) before handing it to the same blueprint:
//! let app = TestApp::boot(example_app::app).await;
//! ```
//!
//! The inferred HList state type cannot be named by user code, so the
//! blueprint returns `impl BootableApp`; the trait exposes exactly what the
//! two consumers need.

use std::future::Future;

use super::*;

/// Assembled application, ready to serve or to be dissected by a test
/// harness. Implemented by the typed [`AppBuilder<T>`].
pub trait BootableApp: Sized {
    /// The resolved bean graph (test harnesses read beans out of it by type).
    fn bean_context(&self) -> Arc<crate::beans::BeanContext>;

    /// The loaded [`R2eConfig`](crate::config::R2eConfig), if any.
    fn r2e_config(&self) -> Option<crate::config::R2eConfig>;

    /// Assemble the final router (in-process test entry point).
    fn into_router(self) -> crate::http::Router;

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

    fn serve(self, addr: &str) -> impl Future<Output = Result<(), Box<dyn std::error::Error>>> {
        AppBuilder::serve(self, addr)
    }

    fn serve_auto(self) -> impl Future<Output = Result<(), Box<dyn std::error::Error>>> {
        AppBuilder::serve_auto(self)
    }
}
