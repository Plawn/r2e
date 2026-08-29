//! Dev-mode support endpoints.
//!
//! When enabled via `.plugin(DevReload)`, the server exposes:
//! - `GET /__r2e_dev/status` — Returns `"dev"` so tooling/scripts can
//!   detect that the server is running in dev mode.
//! - `GET /__r2e_dev/ping` — Returns a timestamp; can be polled by a
//!   browser script to detect when the server has restarted (the PID or
//!   boot-time changes).
//!
//! Pair with `r2e dev`, which supervises `dx serve --hot-patch`. Ordinary app
//! changes are patched into the running binary; cold changes to `env.rs`,
//! `src/env/**`, `Cargo.toml`, or `build.rs` restart the child process. Clients
//! polling `/__r2e_dev/ping` can detect those full restarts and refresh.

use crate::http::header::CONNECTION;
use crate::http::header::{HeaderValue, CACHE_CONTROL};
use crate::http::middleware::Next;
use crate::http::response::IntoResponse;
use crate::http::routing::get;
use crate::http::Request;
use crate::http::Response;
use crate::http::Router;
use std::sync::OnceLock;
use std::time::SystemTime;

#[cfg(feature = "dev-reload")]
use std::any::Any;
#[cfg(feature = "dev-reload")]
use std::collections::HashMap;
#[cfg(feature = "dev-reload")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "dev-reload")]
use std::sync::Mutex;

#[cfg(feature = "dev-reload")]
static LISTENER_STORE: OnceLock<Mutex<HashMap<String, std::net::TcpListener>>> = OnceLock::new();

/// Whether this process is actually running inside the Subsecond hot-patch
/// loop (set by `r2e::launch!` before entering it).
///
/// The dev-reload state caches are process-global; they must engage ONLY
/// under the loop. Merely compiling with the `dev-reload` feature — e.g.
/// `cargo test --features dev-reload`, or an example's feature passthrough —
/// must keep every `build_state()` cold, or unrelated builds in one test
/// process would serve each other's cached graphs.
#[cfg(feature = "dev-reload")]
static HOT_RELOAD_LOOP: AtomicBool = AtomicBool::new(false);

/// Mark this process as running the Subsecond hot-patch loop, enabling the
/// dev-reload state caches (bean-graph fingerprinting, instance reuse,
/// lifecycle skip). Called by `r2e::launch!` before entering the loop.
#[cfg(feature = "dev-reload")]
pub fn mark_hot_reload_loop() {
    HOT_RELOAD_LOOP.store(true, Ordering::Release);
}

/// Undo [`mark_hot_reload_loop`] — **tests only**.
///
/// A test binary that drives hot-patch cycles by hand marks the loop, which is
/// a one-way switch in production (`r2e::launch!` marks it once and never
/// leaves the loop). Tests that need to assert the *production* path — where
/// every `build_state()` is cold and every `load_config` builds its own
/// `LiveConfigRegistry` — flip it back. Take the same serial lock the other
/// dev-reload tests use before calling this.
#[cfg(feature = "dev-reload")]
#[doc(hidden)]
pub fn unmark_hot_reload_loop() {
    HOT_RELOAD_LOOP.store(false, Ordering::Release);
}

/// Whether [`mark_hot_reload_loop`] has been called in this process.
#[cfg(feature = "dev-reload")]
pub(crate) fn hot_reload_loop_active() -> bool {
    HOT_RELOAD_LOOP.load(Ordering::Acquire)
}

/// Retrieve a cached listener for the given address, or bind a new one.
///
/// On first call for a given address, binds a `TcpListener`, stores it, and
/// returns a `try_clone()`. Subsequent calls (after hot-patch) return another
/// clone of the same listener, avoiding port conflicts.
/// The default port used by the Dioxus devserver (`dx serve`).
///
/// When `dx serve --hot-patch` is running, it listens on this port and
/// silently proxies/intercepts HTTP traffic. If the R2E app binds to the
/// same port, requests never reach the real application.
#[cfg(feature = "dev-reload")]
const DIOXUS_DEVSERVER_PORT: u16 = 8080;

/// Extract the port number from an address string like `"0.0.0.0:3000"`.
#[cfg(feature = "dev-reload")]
fn parse_port(addr: &str) -> Option<u16> {
    addr.rsplit(':').next().and_then(|p| p.parse().ok())
}

#[cfg(feature = "dev-reload")]
pub(crate) fn get_or_bind_listener(
    addr: &str,
) -> Result<crate::rt::TcpListener, crate::beans::BootError> {
    // Guard: prevent binding to the Dioxus devserver port.
    if let Some(port) = parse_port(addr) {
        if port == DIOXUS_DEVSERVER_PORT {
            return Err(format!(
                "Cannot bind to port {port} in dev-reload mode: \
                 the Dioxus devserver (`dx serve`) uses this port. \
                 Your requests would be silently intercepted and never reach your app. \
                 Use a different port, e.g. .serve(\"0.0.0.0:3000\")"
            )
            .into());
        }
    }

    let store = LISTENER_STORE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = store
        .lock()
        .map_err(|e| format!("listener store poisoned: {e}"))?;
    if let Some(existing) = map.get(addr) {
        Ok(crate::rt::TcpListener::from_std(existing.try_clone()?)?)
    } else {
        let l = std::net::TcpListener::bind(addr)?;
        l.set_nonblocking(true)?;
        let cloned = l.try_clone()?;
        map.insert(addr.to_string(), l);
        Ok(crate::rt::TcpListener::from_std(cloned)?)
    }
}

// ── QUIC endpoint cache for dev-reload ─────────────────────────────────────

#[cfg(all(feature = "dev-reload", feature = "quic"))]
static QUIC_ENDPOINT_STORE: OnceLock<Mutex<HashMap<String, crate::http::quic::quinn::Endpoint>>> =
    OnceLock::new();

/// Retrieve a cached QUIC endpoint for the given address, or bind a new one.
///
/// On first call, binds a `quinn::Endpoint`, stores it, and returns a clone.
/// Subsequent calls (after hot-patch) return another clone of the same
/// endpoint — same UDP socket, no port conflicts.
///
/// Unlike the TCP [`get_or_bind_listener`], the endpoint is never closed
/// between hot-reload cycles; the accept loop just stops and restarts with
/// the new router.
#[cfg(all(feature = "dev-reload", feature = "quic"))]
pub(crate) fn get_or_bind_quic_endpoint(
    addr: std::net::SocketAddr,
    server_config: crate::http::quic::quinn::ServerConfig,
) -> Result<crate::http::quic::quinn::Endpoint, Box<dyn std::error::Error + Send + Sync>> {
    let key = addr.to_string();
    let store = QUIC_ENDPOINT_STORE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = store
        .lock()
        .map_err(|e| format!("QUIC endpoint store poisoned: {e}"))?;
    if let Some(existing) = map.get(&key) {
        tracing::debug!(addr = %key, "dev-reload: reusing cached QUIC endpoint (cert changes require full restart)");
        Ok(existing.clone())
    } else {
        let endpoint = crate::http::quic::quinn::Endpoint::server(server_config, addr)?;
        map.insert(key, endpoint.clone());
        Ok(endpoint)
    }
}

// ── State cache for dev-reload ──────────────────────────────────────────────

#[cfg(feature = "dev-reload")]
static STATE_CACHE: OnceLock<Mutex<Option<Box<dyn Any + Send + Sync>>>> = OnceLock::new();

#[cfg(feature = "dev-reload")]
static LIFECYCLE_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Retrieve a previously cached application state, if any.
///
/// Returns `Some(T)` on subsequent dev-reload cycles (hot-patches) so beans
/// are not re-resolved. Returns `None` on the very first run.
#[cfg(feature = "dev-reload")]
pub(crate) fn get_cached_state<T: Clone + Send + Sync + 'static>() -> Option<T> {
    let store = STATE_CACHE.get_or_init(|| Mutex::new(None));
    let guard = store.lock().ok()?;
    guard
        .as_ref()
        .and_then(|boxed| boxed.downcast_ref::<T>())
        .cloned()
}

// ── Live-config registry carrier ────────────────────────────────────────────

/// The process-stable [`LiveConfigRegistry`](crate::config::LiveConfigRegistry)
/// for the hot-patch loop.
///
/// A `LiveConfig<T>` handle binds **one slot of one registry** at construction
/// and never looks the registry up again, so a fresh registry per hot-patch
/// cycle would strand every handle built by an earlier cycle. Carrying the
/// instance here gives the registry a single identity per process; each cycle's
/// `load_config` re-seeds it (`LiveConfigRegistry::reseed`) instead of
/// replacing it.
///
/// Single-slot like [`STATE_CACHE`]/[`CTX_CACHE`], and engaged only under
/// [`hot_reload_loop_active`] — outside the loop (production, and any test that
/// never marks the loop) `load_config` builds a fresh registry exactly as
/// before.
#[cfg(feature = "dev-reload")]
static LIVE_CONFIG_REGISTRY: OnceLock<Mutex<Option<crate::config::LiveConfigRegistry>>> =
    OnceLock::new();

/// The registry carried over from an earlier hot-patch cycle, if any.
///
/// Always `None` outside the hot-patch loop, so every non-dev `load_config`
/// keeps building its own registry.
#[cfg(feature = "dev-reload")]
pub(crate) fn carried_live_config_registry() -> Option<crate::config::LiveConfigRegistry> {
    if !hot_reload_loop_active() {
        return None;
    }
    let store = LIVE_CONFIG_REGISTRY.get_or_init(|| Mutex::new(None));
    let guard = store.lock().ok()?;
    guard.clone()
}

/// Carry `registry` into the next hot-patch cycle. No-op outside the loop.
#[cfg(feature = "dev-reload")]
pub(crate) fn carry_live_config_registry(registry: &crate::config::LiveConfigRegistry) {
    if !hot_reload_loop_active() {
        return;
    }
    let store = LIVE_CONFIG_REGISTRY.get_or_init(|| Mutex::new(None));
    let mut guard = store.lock().expect("live config registry carrier poisoned");
    *guard = Some(registry.clone());
}

/// The previous cycle's resolved [`BeanContext`](crate::beans::BeanContext),
/// cached independently of the provision-list type `P` (unlike
/// [`STATE_CACHE`], which is keyed by the monomorphized state tuple). This
/// lets the partial rebuild reuse unchanged bean instances even when the
/// provision list itself changed shape (e.g. a `.provide()` was added).
#[cfg(feature = "dev-reload")]
static CTX_CACHE: OnceLock<Mutex<Option<std::sync::Arc<crate::beans::BeanContext>>>> =
    OnceLock::new();

/// Retrieve the previous cycle's resolved bean context, if any.
#[cfg(feature = "dev-reload")]
pub(crate) fn get_cached_ctx() -> Option<std::sync::Arc<crate::beans::BeanContext>> {
    let store = CTX_CACHE.get_or_init(|| Mutex::new(None));
    let guard = store.lock().ok()?;
    guard.clone()
}

/// Cache the resolved bean context for the next dev-reload cycle.
#[cfg(feature = "dev-reload")]
pub(crate) fn cache_ctx(ctx: &std::sync::Arc<crate::beans::BeanContext>) {
    let store = CTX_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = store.lock().expect("ctx cache poisoned");
    *guard = Some(std::sync::Arc::clone(ctx));
}

/// Returns `true` if lifecycle hooks (consumers, serve hooks, startup hooks)
/// have already been executed in a previous dev-reload cycle. Always `false`
/// outside the hot-patch loop: a test process building several apps must run
/// every app's lifecycle, not skip all but the first.
#[cfg(feature = "dev-reload")]
pub(crate) fn is_lifecycle_initialized() -> bool {
    hot_reload_loop_active() && LIFECYCLE_INITIALIZED.load(Ordering::Acquire)
}

/// Mark lifecycle hooks as having been executed.
#[cfg(feature = "dev-reload")]
pub(crate) fn mark_lifecycle_initialized() {
    LIFECYCLE_INITIALIZED.store(true, Ordering::Release);
}

// ── Staged cycle (two-phase cache commit) ───────────────────────────────────

/// The state/context/fingerprints a `try_build_state()` produced this cycle,
/// held OUT of the live caches until the whole cycle is known to have
/// assembled.
///
/// `try_build_state()` is only the first half of a hot-patch cycle: the
/// enclosing `App::build` can still fail after it (a plugin, a controller, a
/// `?` in the app's own assembly). Committing the caches inside
/// `try_build_state` made a failed cycle leave its graph behind — the beans it
/// built never dropped, and the *next* patch happily reused that failed graph
/// while `LIFECYCLE_INITIALIZED` was still set, skipping its startup
/// lifecycle. Staging makes the commit atomic with cycle success:
/// [`commit_dev_cycle`] on `Ok`, [`rollback_dev_cycle`] on `Err` (which drops
/// the staged `Arc<BeanContext>`, releasing the beans built this cycle).
#[cfg(feature = "dev-reload")]
struct StagedCycle {
    state: Box<dyn Any + Send + Sync>,
    ctx: std::sync::Arc<crate::beans::BeanContext>,
    fingerprint: u64,
    per_bean: crate::beans::BeanFingerprints,
}

#[cfg(feature = "dev-reload")]
static STAGED_CYCLE: OnceLock<Mutex<Option<StagedCycle>>> = OnceLock::new();

/// Stage this cycle's resolved graph. Visible to nothing until
/// [`commit_dev_cycle`]; replaced wholesale if another `build_state()` runs
/// in the same cycle.
#[cfg(feature = "dev-reload")]
pub(crate) fn stage_cycle<T: Clone + Send + Sync + 'static>(
    state: &T,
    ctx: &std::sync::Arc<crate::beans::BeanContext>,
    fingerprint: u64,
    per_bean: crate::beans::BeanFingerprints,
) {
    let store = STAGED_CYCLE.get_or_init(|| Mutex::new(None));
    let mut guard = store.lock().expect("staged cycle poisoned");
    *guard = Some(StagedCycle {
        state: Box::new(state.clone()),
        ctx: std::sync::Arc::clone(ctx),
        fingerprint,
        per_bean,
    });
}

/// Promote the staged cycle into the live dev-reload caches.
///
/// Called by `r2e::launch!` once the hot-patch cycle has assembled
/// successfully (`App::build` returned `Ok`). No-op when nothing is staged —
/// a cache-hit cycle reuses the committed graph and stages nothing.
#[cfg(feature = "dev-reload")]
pub fn commit_dev_cycle() {
    let Some(store) = STAGED_CYCLE.get() else {
        return;
    };
    let staged = match store.lock() {
        Ok(mut guard) => guard.take(),
        Err(_) => return,
    };
    let Some(staged) = staged else { return };

    if let Ok(mut guard) = STATE_CACHE.get_or_init(|| Mutex::new(None)).lock() {
        *guard = Some(staged.state);
    }
    cache_ctx(&staged.ctx);
    cache_graph_fingerprint(staged.fingerprint, staged.per_bean);
}

/// Discard the staged cycle: a hot-patch cycle that failed to assemble leaves
/// the caches exactly as the last successful cycle left them, and the beans it
/// built are dropped here with the staged context.
///
/// Called by `r2e::launch!` when `App::build` returns `Err`.
#[cfg(feature = "dev-reload")]
pub fn rollback_dev_cycle() {
    let Some(store) = STAGED_CYCLE.get() else {
        return;
    };
    if let Ok(mut guard) = store.lock() {
        // Dropped outside the lock: a bean `Drop` must not run while the
        // staging mutex is held (it may itself touch the dev caches).
        let staged = guard.take();
        drop(guard);
        drop(staged);
    }
}

/// Whether a cycle is currently staged but not yet committed — **tests only**.
#[cfg(feature = "dev-reload")]
#[doc(hidden)]
pub fn has_staged_dev_cycle() -> bool {
    STAGED_CYCLE
        .get()
        .and_then(|store| store.lock().ok().map(|g| g.is_some()))
        .unwrap_or(false)
}

/// Force the next dev-reload cycle to rebuild the application state from
/// scratch (re-resolve all beans).
///
/// Call this when you've changed a bean's constructor or initial state and
/// need the change to take effect without a full process restart.
#[cfg(feature = "dev-reload")]
pub fn invalidate_state_cache() {
    if let Some(store) = STATE_CACHE.get() {
        if let Ok(mut guard) = store.lock() {
            *guard = None;
        }
    }
    if let Some(store) = CTX_CACHE.get() {
        if let Ok(mut guard) = store.lock() {
            *guard = None;
        }
    }
    // The carried registry is part of the cache group: "force a cold rebuild"
    // must be cold for live config too, or the next cycle would keep pushing
    // into slots seeded by a session that no longer exists.
    if let Some(store) = LIVE_CONFIG_REGISTRY.get() {
        if let Ok(mut guard) = store.lock() {
            *guard = None;
        }
    }
    invalidate_graph_fingerprint();
    // A staged-but-uncommitted cycle is part of the cache group too: "force a
    // cold rebuild" must not leave one behind to be committed later.
    rollback_dev_cycle();
    LIFECYCLE_INITIALIZED.store(false, Ordering::Release);
}

/// Clear the graph fingerprint cache (used by `invalidate_state_cache`).
#[cfg(feature = "dev-reload")]
fn invalidate_graph_fingerprint() {
    if let Some(store) = GRAPH_FINGERPRINT.get() {
        if let Ok(mut guard) = store.lock() {
            *guard = None;
        }
    }
}

// ── Graph fingerprint cache ─────────────────────────────────────────────────

#[cfg(feature = "dev-reload")]
static GRAPH_FINGERPRINT: OnceLock<Mutex<Option<u64>>> = OnceLock::new();

/// Get the cached graph fingerprint from the previous dev-reload cycle.
#[cfg(feature = "dev-reload")]
pub(crate) fn get_cached_graph_fingerprint() -> Option<u64> {
    let store = GRAPH_FINGERPRINT.get_or_init(|| Mutex::new(None));
    let guard = store.lock().ok()?;
    *guard
}

/// Store the current graph fingerprint and per-bean fingerprints.
#[cfg(feature = "dev-reload")]
pub(crate) fn cache_graph_fingerprint(fp: u64, per_bean: crate::beans::BeanFingerprints) {
    let store = GRAPH_FINGERPRINT.get_or_init(|| Mutex::new(None));
    let mut guard = store.lock().expect("graph fingerprint cache poisoned");
    *guard = Some(fp);

    let bean_store = PER_BEAN_FINGERPRINTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut bean_guard = bean_store
        .lock()
        .expect("per-bean fingerprint cache poisoned");
    bean_guard.clear();
    for (tid, _name, bean_fp) in per_bean {
        bean_guard.insert(tid, bean_fp);
    }
}

#[cfg(feature = "dev-reload")]
static PER_BEAN_FINGERPRINTS: OnceLock<Mutex<HashMap<std::any::TypeId, u64>>> = OnceLock::new();

/// Get the cached per-bean fingerprints from the previous cycle.
#[cfg(feature = "dev-reload")]
pub(crate) fn get_cached_per_bean_fingerprints() -> HashMap<std::any::TypeId, u64> {
    let store = PER_BEAN_FINGERPRINTS.get_or_init(|| Mutex::new(HashMap::new()));
    store.lock().map(|g| g.clone()).unwrap_or_default()
}

// ── Boot time ───────────────────────────────────────────────────────────────

static BOOT_TIME: OnceLock<u64> = OnceLock::new();

fn boot_time() -> u64 {
    *BOOT_TIME.get_or_init(|| {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    })
}

/// Create a router with dev-mode endpoints.
///
/// Intended to be merged into the main application via the
/// [`DevReload`](crate::builtins::DevReload) plugin.
pub fn dev_routes<T: Clone + Send + Sync + 'static>() -> Router<T> {
    Router::new()
        .route("/__r2e_dev/status", get(status_handler))
        .route("/__r2e_dev/ping", get(ping_handler))
}

async fn status_handler() -> impl IntoResponse {
    "dev"
}

async fn ping_handler() -> impl IntoResponse {
    let ts = boot_time();
    serde_json::json!({ "boot_time": ts, "status": "ok" }).to_string()
}

/// Middleware that adds dev-mode headers to every response:
///
/// - `Cache-Control: no-store` — prevents the browser from caching API
///   responses, so Swagger UI always shows fresh data.
/// - `Connection: close` — forces the browser to close the TCP connection
///   after each response. Without this, HTTP keep-alive lets the browser
///   reuse a connection bound to a *previous* server future. When subsecond
///   hot-patches, it drops the old server and starts a new one, but the old
///   connection handler tasks (spawned by the HTTP server) keep running.
///   The browser's keep-alive connection stays routed to stale handlers.
pub async fn dev_headers_middleware(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(CONNECTION, HeaderValue::from_static("close"));
    response
}
