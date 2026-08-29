//! [`RunningApp`]: an app started **in process** — the production lifecycle
//! without a listener.
//!
//! Produced by [`PreparedApp::start_in_process`], which is what
//! `TestApp::boot` boots through. It owns everything the shutdown sequence
//! needs (the app shutdown token, the tracked-handle lane, the
//! drain/`#[pre_destroy]`/`on_stop` hooks, both budgets) with the state type
//! erased, so a test harness can hold it without naming the app's HList state.

use super::*;
use crate::plugin::AsyncShutdownHook;

/// A started, listener-less R2E application.
///
/// `start_in_process()` runs the startup phase (controller
/// `#[post_construct]`, consumer registrations, `#[on_start]`, the builder's
/// `on_start` closures — hence `spawn_service` /
/// `#[derive(BackgroundService)]` tasks); [`shutdown`](Self::shutdown) runs the
/// shutdown phase in the same order and under the same budgets as
/// [`PreparedApp::run`]:
///
/// 1. `on_drain` hooks (unbounded, nothing has been cancelled yet);
/// 2. plugin sync shutdown hooks, then the ordered async disposers (plugin
///    `on_shutdown_async`, controller `#[pre_destroy]`, bean `#[pre_destroy]`);
/// 3. the app shutdown token is cancelled — services stop, and a server
///    attached with [`serve_tracked`](Self::serve_tracked) starts its HTTP
///    drain, bounded by `drain_timeout`;
/// 4. tracked handles are joined, each bounded by `shutdown_grace_period`;
/// 5. `on_stop` hooks — outside every budget, always.
///
/// Dropping without calling `shutdown()` cancels the token (so no background
/// service is stranded) but runs **no** hook: `Drop` cannot await. A drop with
/// hooks still pending logs a warning naming the missing call.
pub struct RunningApp {
    pub(super) router: crate::http::Router,
    /// Strong reference to the resolved graph for the whole in-process
    /// lifetime — the counterpart of `PreparedApp::graph`.
    pub(super) graph: Arc<crate::beans::BeanContext>,
    pub(super) cancel: CancelToken,
    /// Cancel-on-drop for every exit that is not `shutdown()` — a panic in the
    /// test body, an early `return`, a forgotten shutdown. Never read: it
    /// exists for its `Drop`.
    pub(super) _cancel_guard: crate::rt::CancelDropGuard,
    pub(super) plugin_shutdown: super::prepared::PluginShutdownCell,
    pub(super) handles: ServiceHandles,
    /// `on_drain` hooks with the state already bound (see
    /// [`PreparedApp::start_in_process`]).
    pub(super) drain_hooks: Vec<AsyncShutdownHook>,
    pub(super) async_shutdown_hooks: Vec<AsyncShutdownHook>,
    /// `on_stop` hooks with the state already bound.
    pub(super) stop_hooks: Vec<AsyncShutdownHook>,
    pub(super) stop_handle: StopHandle,
    pub(super) shutdown_grace_period: Option<Duration>,
    pub(super) drain_timeout: Option<Duration>,
}

impl RunningApp {
    /// The assembled router (`Clone` — this is what an in-process HTTP client
    /// dispatches against).
    pub fn router(&self) -> &crate::http::Router {
        &self.router
    }

    /// The resolved bean graph.
    pub fn bean_context(&self) -> Arc<crate::beans::BeanContext> {
        Arc::clone(&self.graph)
    }

    /// The app's [`StopHandle`] — the same one a `StopHandle` bean resolves to.
    /// [`shutdown`](Self::shutdown) fires it, so anything awaiting
    /// [`StopHandle::stopped`] observes a real stop.
    pub fn stop_handle(&self) -> StopHandle {
        self.stop_handle.clone()
    }

    /// The app shutdown token: the root every `spawn_service` token is a child
    /// of, cancelled at step 3 of [`shutdown`](Self::shutdown).
    pub fn shutdown_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// The resolved HTTP-drain budget (see [`PreparedApp::drain_timeout`]).
    pub fn drain_timeout(&self) -> Option<Duration> {
        self.drain_timeout
    }

    /// The per-handle bound on the tracked-handle join phase.
    pub fn shutdown_grace_period(&self) -> Option<Duration> {
        self.shutdown_grace_period
    }

    /// Whether any shutdown hook is still pending, i.e. whether
    /// [`shutdown`](Self::shutdown) has something to run. `false` for an app
    /// that registers no `on_drain`/`#[pre_destroy]`/`on_stop` hook — the
    /// harness uses it to keep a hook-less app's teardown free.
    pub fn has_shutdown_work(&self) -> bool {
        !self.drain_hooks.is_empty()
            || !self.async_shutdown_hooks.is_empty()
            || !self.stop_hooks.is_empty()
    }

    /// Spawn `fut` on the app's tracked lane: it owns the bean graph while it
    /// runs, and [`shutdown`](Self::shutdown) joins it under
    /// `shutdown_grace_period`. The in-process equivalent of
    /// [`ServeContext::track_named`].
    pub fn track_named<F>(&self, name: &'static str, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.handles
            .spawn_owning(name, Arc::clone(&self.graph), fut);
    }

    /// Serve this app's router on `listener`, attached to the lifecycle.
    ///
    /// The task lands on the tracked lane and stops accepting on **either**
    /// trigger: the app shutdown token, or `stop_early` (how a live test
    /// server shuts itself down when it is dropped before the app is). Its
    /// HTTP drain is bounded by the app's `drain_timeout` from whichever
    /// trigger fired — not only from the app-wide one — so a `TestServer`
    /// dropped while a request is stuck gives up on schedule instead of
    /// hanging until the whole app is cancelled.
    /// `ConnectInfo<SocketAddr>` is installed like every production serve
    /// path, so peer-address guards behave the same.
    pub fn serve_tracked<F>(&self, listener: crate::rt::TcpListener, stop_early: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        use std::future::IntoFuture as _;
        let svc = self
            .router
            .clone()
            .into_make_service_with_connect_info::<std::net::SocketAddr>();
        let app_cancel = self.cancel.clone();
        // This server's OWN stop token. `bounded_http_drain` starts the
        // `drain_timeout` clock when the token it is given fires, so handing
        // it the app root would leave an early `stop_early` drain unbounded:
        // the budget would only start once someone cancelled the whole app.
        // Cancelling it from inside the graceful-shutdown future arms the
        // clock at exactly the instant the listener stops accepting — the
        // same relationship the production single-listener path has between
        // its shutdown future and the app token.
        let serve_stop = crate::rt::CancelToken::new();
        let arm_deadline = serve_stop.clone();
        let drain_bound = self.drain_timeout;
        self.track_named("in-process http server", async move {
            let serve = crate::http::serve(listener, svc)
                .with_graceful_shutdown(async move {
                    crate::rt::select! {
                        _ = app_cancel.cancelled() => {}
                        _ = stop_early => {}
                    }
                    arm_deadline.cancel();
                })
                .into_future();
            if let Err(e) =
                crate::runtime::drain::bounded_http_drain(serve, serve_stop, drain_bound).await
            {
                tracing::error!(error = %e, "in-process server failed");
            }
        });
    }

    /// Run the graceful-shutdown sequence — the same one `run()` runs on a
    /// signal, in the same order and under the same budgets (see the type
    /// documentation for the five phases).
    pub async fn shutdown(mut self) {
        // Fire the stop handle first: anything watching it (an admin endpoint,
        // a service awaiting `StopHandle::stopped`) sees the same trigger a
        // production stop fires, and it happens before the drain hooks, just
        // like the `select!` in `run_inner`'s shutdown future.
        self.stop_handle.stop();

        // 1. Drain hooks — still "accepting", nothing cancelled yet.
        for hook in std::mem::take(&mut self.drain_hooks) {
            hook().await;
        }

        // 2. Plugin sync hooks (they cancel the per-service tokens early),
        //    then the ordered async disposers.
        self.plugin_shutdown.fire();
        for hook in std::mem::take(&mut self.async_shutdown_hooks) {
            hook().await;
        }

        // 3. Cancel the root: services stop, an attached server stops
        //    accepting and starts its `drain_timeout`-bounded drain.
        self.cancel.cancel();

        // 4. Join the tracked handles, each bounded by
        //    `shutdown_grace_period` (a service that ignores its token is
        //    abandoned with a warning naming it, not waited on forever).
        super::prepared::drain_tracked_handles(&self.handles, self.shutdown_grace_period).await;

        // 5. `on_stop` hooks — MUST-RUN, outside every budget.
        for hook in std::mem::take(&mut self.stop_hooks) {
            hook().await;
        }
    }
}

impl Drop for RunningApp {
    /// Cancels the app token (the `cancel_guard` field does it as this value is
    /// dropped), so no background service outlives the app — but nothing is
    /// awaited: `Drop` cannot run the async shutdown sequence. A drop with
    /// hooks still pending says so, once, rather than silently skipping the
    /// app's shutdown contract.
    fn drop(&mut self) {
        if self.has_shutdown_work() {
            tracing::warn!(
                "app dropped without `shutdown().await`: on_drain, #[pre_destroy] \
                 and on_stop hooks did not run (background services were still \
                 cancelled). Call `.shutdown().await` to exercise the shutdown \
                 sequence."
            );
        }
        // `_cancel_guard` fires here.
    }
}
