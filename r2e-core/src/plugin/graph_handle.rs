#[allow(unused_imports)] // referenced by intra-doc links
use super::{PluginBuildContext, PreStatePlugin};

// ── Plugin build machinery ─────────────────────────────────────────────────

/// A cheap, cloneable, **deferred-fill** handle on the final resolved bean
/// graph.
///
/// Handed to plugins via [`PluginBuildContext::graph`] while the graph is
/// still being built; the framework fills it right after `build_state()`
/// resolves, so [`get`](Self::get) returns `Some` from code running after
/// resolution — serve hooks, request handlers, tracked background tasks (see
/// the ownership rules below for the exact extent). Reading it **during** a
/// plugin's `build` returns `None` — take dependencies through
/// [`Deps`](PreStatePlugin::Deps) instead; the handle exists for values that
/// must resolve beans lazily *after* boot (per-tenant sources, resource
/// factories).
///
/// # The reference is **weak**
///
/// The handle stores a [`Weak`](std::sync::Weak) reference. It has to: the
/// beans it points at typically live *inside* the very graph it points to
/// (`BeanContext → Tenanted<T> → GraphHandle`), and a strong reference there
/// would be a cycle that keeps every bean, pool and connection alive forever —
/// one leaked graph per `r2e dev` hot-patch cycle.
///
/// # Who keeps it alive
///
/// Strong ownership sits with the assembled app, in three independent places —
/// no one of them is a link in a chain, each stands alone:
///
/// - **the router**, through its `GraphKeepAlive` layer: every request future
///   and every response body carries the graph, so a handler that resolves
///   after an `.await` — or a streaming body producing frames long after the
///   handler returned — still sees it;
/// - **every tracked task**: `ServeContext::track`, `spawn_service`, the
///   scheduler driver and the QUIC drain move an `Arc` *into* the task.
///   `run()` cancels the shutdown token and joins those handles on every exit
///   it controls — normal shutdown *and* the aborts (a startup hook returning
///   `Err`, a serve error) — but that join is still bounded by
///   `shutdown_grace_period`, and a dropped `run()` future (an `r2e dev` hot
///   patch) joins nothing at all, so ownership travels with the work;
/// - **the serving scope**: `PreparedApp::run()` holds one for its whole
///   duration, covering the shutdown phase itself (`on_stop` hooks,
///   `#[pre_destroy]` disposers).
///
/// `get`/[`bean`](Self::bean) therefore return `Some` for the app's whole life
/// and for any tracked task that outlives it. They read empty in four
/// situations:
///
/// - **not filled yet** — during `build`, or after a `build_state()` that
///   failed (see [`fill`](Self::fill));
/// - **graph already dropped** — the app that owned it is gone (a handle kept
///   by hand past the app is the usual way to hit this);
/// - **a WebSocket session still running after `run()` returned** — upgraded
///   connections are detached from graceful shutdown and are not tracked, so
///   resolve what a session needs before entering its socket loop;
/// - **an `r2e dev` hot patch, once the previous cycle has fully wound down** —
///   dropping the old `run()` future cancels that cycle's shutdown token, so
///   its tracked tasks stop; each keeps *its own* cycle's graph alive until it
///   returns (nothing joins them), and that graph is released when the last one
///   does. A handle carried into the new cycle points at the old, released
///   graph.
///
/// That last point (and a panic unwinding out of the serve loop) relies on
/// cancellation alone, since no shutdown hook runs on either path: the app
/// shutdown token is created before serving (lazily, on the first
/// `register_service` or in `run_inner`) and shared through `plugin_data`, and
/// every framework-derived token — `spawn_service`'s per-service token in
/// particular — is a `child_token()` of it, so the drop guard on the app token
/// reaches them all. Tokens the framework does not own (the scheduler's, a
/// plugin's) need the explicit relay the scheduler does.
///
/// Code that may outlive the graph should treat `None` as "the app is shutting
/// down", not as a bug.
#[derive(Clone, Default)]
pub struct GraphHandle(crate::di::late::Late<std::sync::Weak<crate::beans::BeanContext>>);

impl GraphHandle {
    /// Create an empty (unfilled) handle. Internal — the builder owns filling.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Fill the handle with a weak reference to the resolved graph. First write
    /// wins; later calls are ignored (relevant across dev-reload cycles, where
    /// the registry — and thus the handle — is fresh per cycle anyway).
    ///
    /// The builder does this for you on every **successful** exit of
    /// `build_state()`. A boot that fails (bean error, plugin `build` error)
    /// returns before the fill, so a handle held outside the builder stays
    /// empty forever — there is no graph to point at.
    ///
    /// It is public for embedders that build a `BeanContext` by hand (tests,
    /// hand-wired per-tenant maps) and need to satisfy an API that takes a
    /// `GraphHandle`: start from [`GraphHandle::default`], hand out clones,
    /// fill once. The caller keeps owning the `Arc` — the handle will not.
    pub fn fill(&self, ctx: &std::sync::Arc<crate::beans::BeanContext>) {
        let _ = self.0.fill(std::sync::Arc::downgrade(ctx));
    }

    /// The resolved bean graph, or `None` before `build_state()` completes —
    /// and `None` again once the app owning the graph has been dropped.
    pub fn get(&self) -> Option<std::sync::Arc<crate::beans::BeanContext>> {
        self.0.get().and_then(std::sync::Weak::upgrade)
    }

    /// Resolve a bean from the graph, or `None` before resolution / after the
    /// graph is gone / when the bean is absent.
    pub fn bean<B: Clone + Send + Sync + 'static>(&self) -> Option<B> {
        self.get().and_then(|ctx| ctx.try_get::<B>())
    }
}

impl std::fmt::Debug for GraphHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphHandle")
            .field("filled", &self.0.get().is_some())
            .field("alive", &self.get().is_some())
            .finish()
    }
}
