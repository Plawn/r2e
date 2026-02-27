//! Subsecond hot-reload integration for R2E.
//!
//! This crate should only be used in development via the `dev-reload` feature flag.
//! It wraps Dioxus's Subsecond engine to enable hot-patching of Rust code at runtime.

pub use dioxus_devtools;

/// Run an R2E app with Subsecond hot-reloading, using a setup closure.
///
/// # Arguments
///
/// * `setup_fn` — Called **once** at startup. Use this for DB connections, config loading,
///   tracing init, and anything expensive. The returned `Env` is passed to every invocation
///   of `server_fn` and persists across hot-patches.
///
/// * `server_fn` — Called on every hot-patch. This is where you build your `AppBuilder`,
///   register controllers, and call `.serve()`. This function's code (and everything it
///   calls in the tip crate) will be hot-patched when source files change.
///
/// # Example
///
/// ```rust,no_run
/// # use r2e_devtools::serve_with_hotreload;
/// # #[derive(Clone)]
/// # struct AppEnv { config: (), db: () }
/// # fn load_config() -> () {}
/// # async fn setup_db() -> () {}
/// # async fn build_and_serve(_: AppEnv) {}
/// # async fn example() {
/// serve_with_hotreload(
///     || async {
///         let config = load_config();
///         let db = setup_db().await;
///         AppEnv { config, db }
///     },
///     |env| async move {
///         build_and_serve(env).await;
///     },
/// ).await;
/// # }
/// ```
pub async fn serve_with_hotreload<Env, SetupFut, ServerFn, ServerFut>(
    setup_fn: impl FnOnce() -> SetupFut,
    server_fn: ServerFn,
) where
    Env: Clone + Send + Sync + 'static,
    SetupFut: std::future::Future<Output = Env>,
    ServerFn: Fn(Env) -> ServerFut + Send + Sync + 'static,
    ServerFut: std::future::Future<Output = ()> + 'static,
{
    let env = setup_fn().await;
    serve_with_hotreload_env(env, server_fn).await;
}

/// Run an R2E app with Subsecond hot-reloading, using a pre-built environment.
///
/// Like [`serve_with_hotreload`], but takes a pre-built `Env` directly instead
/// of a setup closure. This is the core implementation that both
/// `serve_with_hotreload` and `AppBuilder::serve_hot` delegate to.
///
/// # Important
///
/// `server_fn` should be a non-capturing closure or a named function — NOT
/// wrapped in `Arc` or other pointer-sized wrappers. Subsecond's `HotFn`
/// dispatches differently for pointer-sized vs zero-sized callables, and
/// wrapping in `Arc` makes the closure look like a function pointer, causing
/// jump-table lookups to fail and fall back to stale code.
///
/// # Arguments
///
/// * `env` — Pre-computed environment (DB pool, config, etc.) that persists
///   across hot-patches.
/// * `server_fn` — Called on every hot-patch.
pub async fn serve_with_hotreload_env<Env, ServerFn, ServerFut>(
    env: Env,
    server_fn: ServerFn,
) where
    Env: Clone + Send + Sync + 'static,
    ServerFn: Fn(Env) -> ServerFut + Send + Sync + 'static,
    ServerFut: std::future::Future<Output = ()> + 'static,
{
    // Pass the server_fn directly to serve_subsecond_with_args — do NOT wrap
    // it in Arc. Fn implies FnMut, so this satisfies the FnMut bound.
    //
    // Wrapping in Arc<ServerFn> makes the closure pointer-sized (8 bytes),
    // which triggers subsecond's `call_as_ptr` path. That path transmutes
    // the Arc pointer as a function address, fails the jump table lookup,
    // and falls back to calling the original (stale) code.
    dioxus_devtools::serve_subsecond_with_args(env, server_fn).await;
}
