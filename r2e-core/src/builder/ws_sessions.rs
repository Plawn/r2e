//! [`WsSessions`]: the tracked lane for upgraded WebSocket sessions.
//!
//! An upgraded WebSocket is invisible to both shutdown budgets unless someone
//! puts it back in view. For hyper the connection is *finished* the moment the
//! upgrade is handed over — so the HTTP drain (`drain_timeout`) does not wait
//! for it — and the future axum spawns for `on_upgrade` is detached, so the
//! tracked-handle join (`shutdown_grace_period`) never sees it either. Without
//! this module a session is killed by runtime teardown after `run()` returns:
//! no close frame, no chance to reconcile per-session state.
//!
//! `WsSessions` closes that gap. It is a plain bean (provided by
//! [`AppBuilder::new`](crate::AppBuilder::new), so it is in every graph built
//! through `build_state()`), resolved **once at registration time** by the
//! `#[ws(...)]` route closure and armed at serve time by `PreparedApp::run()`
//! with the app's shutdown token and the shared tracked-handle collector.
//!
//! Armed, [`run_session`](WsSessions::run_session) moves the session body onto
//! the tracked lane: it is spawned through the same `spawn_owning` machinery as
//! `spawn_service` / `ServeContext::track`, so it owns the bean graph while it
//! runs and its handle is joined after the HTTP drain, bounded on its own by
//! `shutdown_grace_period` and named `ws:<Controller>::<method>` in the
//! grace-period warning.
//!
//! Unarmed — an app built with `build_with_consumers()` / `TestApp`, a
//! `with_state()` app that has no bean graph, or any router served outside
//! `run()` — the body simply runs inline in axum's detached task, exactly as
//! before this module existed. Nothing panics and nothing changes.

use std::future::Future;
use std::sync::{Arc, RwLock, Weak};

use super::ServiceHandles;
use crate::beans::BeanContext;
use crate::rt::CancelToken;

/// What a serving app hands to [`WsSessions::arm`].
#[derive(Clone)]
struct WsArm {
    /// The shared post-drain handle collector — the same instance
    /// `spawn_service` and `ServeContext::track` push into.
    handles: ServiceHandles,
    /// The app shutdown token, cancelled when the graceful drain begins.
    shutdown: CancelToken,
    /// Weak on purpose: this value lives *inside* the graph it points at
    /// (`BeanContext -> WsSessions -> BeanContext` would be a cycle keeping
    /// every bean alive for the process's lifetime, one per `r2e dev` cycle).
    /// It is upgraded once per session, and the resulting `Arc` is moved into
    /// the tracked task — which is what keeps the graph alive *for* the
    /// session.
    graph: Weak<BeanContext>,
}

/// Registry of live WebSocket sessions, resolvable as a bean.
///
/// Provided automatically by [`AppBuilder::new`](crate::AppBuilder::new); the
/// generated `#[ws(...)]` handler resolves it from the bean context at
/// registration time and every upgraded session runs through it. See the
/// [module docs](self) for why upgraded sockets need this at all.
///
/// Cloning is cheap (one `Arc`) and every clone shares one armed/unarmed
/// state — the value in the graph, the clones captured by route closures, and
/// the one `run()` arms are all the same registry.
#[derive(Clone, Default)]
pub struct WsSessions(Arc<RwLock<Option<WsArm>>>);

impl WsSessions {
    /// Point this registry at a serving app's tracked lane.
    ///
    /// Called once per `run()`, before the serve hooks. Re-arming is
    /// deliberate: under `r2e dev` the bean graph (and with it this registry)
    /// can be carried across hot-patch cycles, and a session must never be
    /// tracked against the *previous* cycle's handles nor cancelled by the
    /// previous cycle's token.
    pub(super) fn arm(
        &self,
        handles: ServiceHandles,
        shutdown: CancelToken,
        graph: &Arc<BeanContext>,
    ) {
        *self.0.write().expect("WsSessions lock poisoned") = Some(WsArm {
            handles,
            shutdown,
            graph: Arc::downgrade(graph),
        });
    }

    /// Stop tracking: sessions opened after this point run inline again.
    ///
    /// Called at the end of the shutdown phase, once the tracked handles have
    /// been joined — a handle pushed after the join would never be awaited.
    pub(super) fn disarm(&self) {
        *self.0.write().expect("WsSessions lock poisoned") = None;
    }

    /// Whether a serving app has claimed this registry.
    ///
    /// `false` in a `TestApp` / `build_with_consumers()` app, where sessions
    /// run untracked.
    pub fn is_armed(&self) -> bool {
        self.0.read().expect("WsSessions lock poisoned").is_some()
    }

    /// Resolve the registry from a bean context, falling back to an unarmed one
    /// when the app has no bean graph (the `with_state()` path).
    ///
    /// Called by generated `#[ws(...)]` route code at registration time.
    #[doc(hidden)]
    pub fn from_context(ctx: &BeanContext) -> Self {
        ctx.try_get::<Self>().unwrap_or_default()
    }

    /// Run one upgraded WebSocket session, on the tracked lane when armed.
    ///
    /// `label` names the session in the `shutdown_grace_period` warning; the
    /// generated code passes `ws:<Controller>::<method>`. `body` receives the
    /// app shutdown token (`None` when unarmed) — that is what
    /// [`WsStream`](crate::web::ws::WsStream) observes to end the session with
    /// a `1001 Going Away` close frame.
    ///
    /// Armed, this **returns as soon as the session is spawned**: the socket
    /// now lives in a tracked task and axum's detached upgrade task has nothing
    /// left to do. Unarmed, the body is awaited inline.
    ///
    /// The tracked task is spawned with [`spawn_ctl`](crate::rt::spawn_ctl):
    /// under SO_REUSEPORT sharded serving the handler runs on a worker's
    /// `current_thread` runtime, which is torn down when that worker leaves its
    /// serve loop — i.e. *before* the tracked handles are joined. The control
    /// plane (the main runtime) outlives the workers, so that is where a
    /// session belongs.
    #[doc(hidden)]
    pub async fn run_session<F, Fut>(self, label: &'static str, body: F)
    where
        F: FnOnce(Option<CancelToken>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let arm = self.0.read().expect("WsSessions lock poisoned").clone();
        // A `graph.upgrade()` that fails means the app which armed us is
        // already gone; running inline is the honest fallback (there is no
        // graph left to own, and nobody left to join the handle).
        let armed =
            arm.and_then(|a| a.graph.upgrade().map(|graph| (a.handles, a.shutdown, graph)));
        match armed {
            Some((handles, shutdown, graph)) => {
                handles.spawn_owning_ctl(label, graph, body(Some(shutdown)));
            }
            None => body(None).await,
        }
    }
}

impl std::fmt::Debug for WsSessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsSessions")
            .field("armed", &self.is_armed())
            .finish()
    }
}
