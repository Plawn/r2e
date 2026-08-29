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
//!     async fn setup() -> Result<DbPool, BootError> {
//!         Ok(DbPool::connect().await?)
//!     }
//!
//!     async fn build(b: AppBuilder, env: DbPool) -> Result<impl BootableApp, BootError> {
//!         Ok(b.provide(env)
//!             .load_config::<AppConfig>()
//!             .register::<UserService>()
//!             .plugin(Health)
//!             .try_build_state().await?
//!             .register_controllers::<(UserController,)>())
//!     }
//! }
//!
//! // Simple apps with no long-lived resources:
//! //   type Env = ();
//! //   async fn setup() -> Result<(), BootError> { Ok(()) }
//!
//! // ── lib.rs ─────────────────────────────────────────────────────────
//! include!("app.rs");
//!
//! // ── main.rs ────────────────────────────────────────────────────────
//! r2e::app_main!(MyApp);
//!
//! // ── a test ─────────────────────────────────────────────────────────
//! #[r2e::test(app = MyApp)]
//! async fn lists_users(app: TestApp) {
//!     app.get("/users").as_user("alice", &["admin"]).send().await.assert_ok();
//! }
//! ```

use std::future::Future;

use super::{AppBuilder, BootableApp};
use crate::beans::BootError;

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
    ///
    /// Fallible: connecting a pool, taking an instance lock, reading a secret.
    /// An `Err` aborts boot — [`launch`] prints it and exits non-zero,
    /// `TestApp::boot` turns it into a failing test naming the cause. **Never
    /// call [`std::process::exit`] here**: `setup` is library code that the
    /// test harness links, and an `exit` there kills the whole test binary
    /// (no attributable failure, no `Drop` for anything already built).
    ///
    /// An app that cannot fail returns `Ok(..)`; the error type is
    /// [`BootError`], so `?` accepts any `std` error.
    fn setup() -> impl Future<Output = Result<Self::Env, BootError>>;

    /// Assemble the application from a fresh [`AppBuilder`] and the environment
    /// produced by [`setup`](App::setup). This is the app's single assembly
    /// path, shared by production, dev-reload, and tests.
    ///
    /// Fallible for the same reason as [`setup`](App::setup). Pair it with
    /// [`try_build_state`](AppBuilder::try_build_state) to surface a bean
    /// graph failure as an error instead of a panic:
    ///
    /// ```ignore
    /// async fn build(b: AppBuilder, env: DbPool) -> Result<impl BootableApp, BootError> {
    ///     Ok(b.provide(env)
    ///         .register::<UserService>()
    ///         .try_build_state().await?
    ///         .register_controllers::<(UserController,)>())
    /// }
    /// ```
    fn build(
        b: AppBuilder,
        env: Self::Env,
    ) -> impl Future<Output = Result<impl BootableApp, BootError>>;
}

/// Run an [`App`] to completion: `setup`, `build`, then serve (reading
/// `server.host`/`server.port` from config, like
/// [`serve_auto`](BootableApp::serve_auto)).
///
/// This is the production entry point. It is invoked for you by `launch!`; the
/// canonical `main.rs` form is:
///
/// ```ignore
/// r2e::app_main!(MyApp);
/// ```
///
/// # Why a macro wraps this in dev mode
///
/// Subsecond (the `dev-reload` hot-patch engine) only remaps function symbols
/// it attributes to the **tip crate** — the crate that owns `main.rs`. A
/// generic dispatcher monomorphised from `r2e-core` (like an earlier
/// `launch::<A>` that drove the loop itself) is *not* remapped: the jump-table
/// lookup misses and hot-patches never reach the rebuilt `App::build`. The
/// `launch!` macro therefore expands the hot-reload loop —
/// including a concrete, named `__r2e_server` function — directly at the call
/// site in the tip crate, which is what makes patches actually apply. Under
/// the standard R2E layout, `app_main!` includes canonical `src/app.rs` in the
/// binary while `lib.rs` includes it for tests. This makes the code reached by
/// the concrete dispatcher tip-crate code without duplicating the declaration.
/// Under
/// `dev-reload` that macro calls [`App::setup`] **once** (its environment
/// survives patches) and re-runs [`App::build`] + serve per hot-patch;
/// `build`'s `load_config` re-reads `application.yaml` per patch so config
/// edits are picked up on the next patch. Without `dev-reload` the macro just
/// calls this function.
/// # Errors
///
/// Returns the first boot failure — [`App::setup`], [`App::build`] (bean
/// construction included, when the app uses
/// [`try_build_state`](AppBuilder::try_build_state)) or serving itself. The
/// caller decides the exit status; `app_main!` prints one line and exits `1`.
pub async fn launch<A: App>() -> Result<(), BootError> {
    let env = A::setup().await?;
    A::build(AppBuilder::new(), env).await?.serve_auto().await
}

/// Render a boot failure as R2E's single operational message: one `error:`
/// line, then one `  caused by:` line per level of the [`source`] chain.
///
/// [`source`]: std::error::Error::source
///
/// A boot failure is an operational condition (a pool that will not connect, a
/// port already taken, a missing secret), not a bug, so the entry point reports
/// it like a CLI tool rather than panicking: no backtrace, no `RUST_BACKTRACE`
/// advice, and exactly one message however deep the cause chain is.
pub fn boot_error_report(err: &BootError) -> String {
    let mut report = format!("error: {err}");
    let mut source = std::error::Error::source(err.as_ref());
    while let Some(cause) = source {
        report.push_str(&format!("\n  caused by: {cause}"));
        source = std::error::Error::source(cause);
    }
    report
}

/// The tail of every R2E entry point: on `Err`, print
/// [`boot_error_report`] to stderr and exit with status `1`; on `Ok`, return.
///
/// This is what `app_main!` wraps around [`launch!`]. A custom `main` that
/// drives `launch!` itself gets the same contract — non-zero status, one
/// message — by ending with:
///
/// ```ignore
/// #[r2e::main]
/// async fn main() {
///     r2e::exit_on_boot_error(r2e::launch!(MyApp).await);
/// }
/// ```
pub fn exit_on_boot_error(result: Result<(), BootError>) {
    if let Err(err) = result {
        eprintln!("{}", boot_error_report(&err));
        std::process::exit(1);
    }
}
