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
//! // ── app.rs (included by lib.rs and by the dev binary) ───────────────
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
//! // ── lib.rs ─────────────────────────────────────────────────────────
//! include!("app.rs");
//!
//! // ── main.rs ────────────────────────────────────────────────────────
//! #[cfg(feature = "dev-reload")]
//! include!("app.rs");
//! #[cfg(not(feature = "dev-reload"))]
//! use my_app::MyApp;
//!
//! #[r2e::main]
//! async fn main() {
//!     r2e::launch!(MyApp).await.unwrap();
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
/// This is the production entry point. It is invoked for you by the
/// [`launch!`](crate::launch) macro, which is the canonical `main.rs` form:
///
/// ```ignore
/// #[r2e::main]
/// async fn main() {
///     r2e::launch!(MyApp).await.unwrap();
/// }
/// ```
///
/// # Why a macro wraps this in dev mode
///
/// Subsecond (the `dev-reload` hot-patch engine) only remaps function symbols
/// it attributes to the **tip crate** — the crate that owns `main.rs`. A
/// generic dispatcher monomorphised from `r2e-core` (like an earlier
/// `launch::<A>` that drove the loop itself) is *not* remapped: the jump-table
/// lookup misses and hot-patches never reach the rebuilt `App::build`. The
/// [`launch!`](crate::launch) macro therefore expands the hot-reload loop —
/// including a concrete, named `__r2e_server` function — directly at the call
/// site in the tip crate, which is what makes patches actually apply. Under
/// the standard R2E layout, `main.rs` also includes the canonical `app.rs`
/// source under `dev-reload`, while `lib.rs` includes it for tests/prod. This
/// makes the code reached by the concrete dispatcher tip-crate code without
/// duplicating the declaration. Under
/// `dev-reload` that macro calls [`App::setup`] **once** (its environment
/// survives patches) and re-runs [`App::build`] + serve per hot-patch;
/// `build`'s `load_config` re-reads `application.yaml` per patch so config
/// edits are picked up on the next patch. Without `dev-reload` the macro just
/// calls this function.
pub async fn launch<A: App>() -> Result<(), Box<dyn std::error::Error>> {
    let env = A::setup().await;
    A::build(AppBuilder::new(), env).await.serve_auto().await
}
