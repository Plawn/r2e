//! [`App`]: the canonical way to declare an R2E application.
//!
//! An `App` bundles the two things every R2E program needs — a one-time
//! **setup** step producing long-lived resources ([`App::Env`]) and a
//! **build** step assembling the [`AppBuilder`] into a servable
//! [`BootableApp`]. It is the single unit consumed uniformly by production
//! serving ([`launch`]), dev-mode hot-reload, and the test harness
//! (`TestApp::boot::<A>()`), replacing the older inline-`main` /
//! blueprint-fn / `#[r2e::main(setup)]` conventions.
//!
//! ```ignore
//! // ── lib.rs ─────────────────────────────────────────────────────────
//! use r2e::prelude::*;
//!
//! pub struct MyApp;
//!
//! impl App for MyApp {
//!     // Resources built once; in dev mode they survive hot-patches.
//!     type Env = DbPool;
//!
//!     async fn setup() -> DbPool {
//!         DbPool::connect().await
//!     }
//!
//!     async fn build(b: AppBuilder, env: DbPool) -> impl BootableApp {
//!         b.provide(env)
//!             .load_config::<AppConfig>()
//!             .register::<UserService>()
//!             .build_state().await
//!             .with(Health)
//!             .register_controllers::<(UserController,)>()
//!     }
//! }
//!
//! // Simple apps with no long-lived resources:
//! //   type Env = ();
//! //   async fn setup() {}
//!
//! // ── main.rs ────────────────────────────────────────────────────────
//! #[r2e::main]
//! async fn main() {
//!     r2e::launch::<MyApp>().await.unwrap();
//! }
//!
//! // ── a test ─────────────────────────────────────────────────────────
//! #[r2e::test(app = MyApp)]
//! async fn lists_users(app: TestApp) {
//!     app.get("/users").as_user("alice", &["admin"]).send().await.assert_ok();
//! }
//! ```

use std::future::Future;

use super::{AppBuilder, BootableApp};

/// The canonical declaration of an R2E application.
///
/// Implement it with `async fn` syntax (RPITIT, Rust >= 1.75). The trait is
/// consumed identically by [`launch`] (production + dev hot-reload) and by the
/// test harness (`TestApp::boot::<A>()`), so an app is declared once and runs
/// the same everywhere.
pub trait App {
    /// Resources provisioned once by [`setup`](App::setup) and passed to every
    /// [`build`](App::build) invocation. In dev mode they are created once and
    /// survive hot-patches (only `build` re-runs per patch).
    ///
    /// Use `()` for apps that own no long-lived setup resources.
    type Env: Clone + Send + Sync + 'static;

    /// Build the long-lived environment. Called once per process (once per
    /// `TestApp::boot` in tests), before [`build`](App::build).
    fn setup() -> impl Future<Output = Self::Env>;

    /// Assemble the application from a fresh [`AppBuilder`] and the environment
    /// produced by [`setup`](App::setup). This is the app's single assembly
    /// path, shared by production, dev-reload, and tests.
    fn build(b: AppBuilder, env: Self::Env) -> impl Future<Output = impl BootableApp>;
}

/// Run an [`App`] to completion: `setup`, `build`, then serve (reading
/// `server.host`/`server.port` from config, like
/// [`serve_auto`](BootableApp::serve_auto)).
///
/// This is THE production entry point. Pair it with a parameterless
/// `#[r2e::main]`:
///
/// ```ignore
/// #[r2e::main]
/// async fn main() {
///     r2e::launch::<MyApp>().await.unwrap();
/// }
/// ```
///
/// With the `dev-reload` feature enabled, [`launch`] runs the app under
/// Subsecond hot-patching: [`App::setup`] is called **once** and its
/// environment is kept alive across patches, while [`App::build`] + serve
/// re-run on every hot-patch. `build`'s `load_config` re-reads
/// `application.yaml` per patch — deliberately, so config file edits are
/// picked up on the next hot-patch instead of serving a stale first-boot
/// config for the whole dev session.
#[cfg(not(feature = "dev-reload"))]
pub async fn launch<A: App>() -> Result<(), Box<dyn std::error::Error>> {
    let env = A::setup().await;
    A::build(AppBuilder::new(), env).await.serve_auto().await
}

/// See the non-dev-reload variant for the full contract.
#[cfg(feature = "dev-reload")]
pub async fn launch<A: App + 'static>() -> Result<(), Box<dyn std::error::Error>> {
    let env = A::setup().await;
    // The closure must stay non-capturing (ZST) so Subsecond's HotFn dispatches
    // through the jump table — everything it needs travels in the `env` arg.
    r2e_devtools::serve_with_hotreload_env(env, |env| __r2e_launch_patch::<A>(env)).await;
    Ok(())
}

/// One hot-patch iteration of the dev-reload [`launch`] loop: rebuild the app
/// on a fresh builder (so `load_config` re-reads YAML — config edits apply on
/// the next patch) and serve it.
#[cfg(feature = "dev-reload")]
async fn __r2e_launch_patch<A: App + 'static>(env: A::Env) {
    let app = A::build(AppBuilder::new(), env).await;
    if let Err(e) = app.serve_auto().await {
        tracing::error!("dev-reload serve failed: {e}");
    }
}
