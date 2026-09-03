//! Application builder: two-phase assembly of an R2E app.
//!
//! - [`nostate`]: the pre-state phase (`AppBuilder<NoState>`) — bean/producer
//!   registration, config loading, plugin installation, `build_state`.
//! - [`typed`]: the typed phase (`AppBuilder<T>`) — controllers, plugins,
//!   layers, hooks, `build()` / `prepare()` / `serve()`.
//! - [`prepared`]: [`PreparedApp`] + the serving lifecycle (`run()`).
//! - [`task_registry`]: [`TaskRegistryHandle`] shared by scheduler/gRPC/plugins.
//! - [`ws_sessions`]: [`WsSessions`] — the tracked lane upgraded WebSocket
//!   sessions run on, so they participate in `shutdown_grace_period`.

mod app;
mod bootable;
mod nostate;
mod prepared;
mod registration;
mod running;
mod task_registry;
mod typed;
#[cfg(feature = "ws")]
mod ws_sessions;

pub use app::{boot_error_report, exit_on_boot_error, launch, launch_with, App, LaunchOptions};
pub use bootable::BootableApp;
pub use prepared::{PreparedApp, PER_WORKER_REQUIRES_SHARDING_MSG};
pub use registration::{
    RegisterController, RegisterControllers, RegisterModule, RegisterModules, SpawnService,
};
pub use running::RunningApp;
pub use task_registry::{ScheduledTaskMarker, TaskRegistryHandle};
#[cfg(feature = "ws")]
pub use ws_sessions::WsSessions;

use crate::beans::{AsyncBean, Bean, BeanRegistry, Producer, Registrable};
use crate::controller::Controller;
use crate::di::meta::MetaRegistry;
use crate::di::module::{
    BeanList, ControllerDepsList, ExportsProvided, FeatureModule, ModEntry, ModuleAggregate,
    ModuleDepsSatisfied, ModuleEndpointSet, ModuleGroup, ModuleList, ModulePluginProvisions,
    ModulePlugins, ModuleProvided, ModuleScope, PushPluginCtrls, RequiredPluginsInstalled,
};
use crate::plugin::{DeferredAction, DeferredContext, PluginInstall, RoutesEffect};
use crate::rt::CancelToken;
use crate::runtime::lifecycle::{DrainHook, ShutdownHook, StartupHook, StopHandle};
use crate::runtime::service::ServiceComponent;
use crate::type_list::{AllSatisfied, BeanState, BuildHList, TAppend, TCons, TNil};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::info;

/// Builder returned by the NoState registration methods
/// ([`register`](AppBuilder::register), [`with_default_bean`](AppBuilder::with_default_bean), …):
/// `Provided` is pushed onto the provision list `P` and `Deps` is appended to
/// the requirement list `R`.
pub type Registered<Provided, Deps, P, R, Mods> =
    AppBuilder<NoState, TCons<Provided, P>, <R as TAppend<Deps>>::Output, Mods>;

/// Builder returned by [`load_config`](AppBuilder::load_config): pushes the
/// typed config `C`, runtime [`LiveConfigRegistry`](crate::config::LiveConfigRegistry),
/// the raw [`R2eConfig`](crate::config::R2eConfig), and `C`'s nested section
/// types (`C::Children`) onto the provision list.
pub type WithLoadedConfig<C, P, R, Mods> = AppBuilder<
    NoState,
    TCons<
        C,
        TCons<
            crate::config::LiveConfigRegistry,
            TCons<
                crate::config::R2eConfig,
                <<C as crate::config::LoadableConfig>::Children as TAppend<P>>::Output,
            >,
        >,
    >,
    R,
    Mods,
>;

/// Builder returned by [`plugin`](AppBuilder::plugin): the plugin's
/// `Provisions` join `P`, its `Required` (`Deps`) joins `R`, and — when it
/// ships any — its `Controllers` are queued on `Mods` so `build_state()`
/// registers them. Nothing is checked at the call site; `Required` is verified
/// against the final provision list at `build_state()`.
pub type WithPluginInstalled<Pl, P, R, Mods> = AppBuilder<
    NoState,
    <P as TAppend<<Pl as PluginInstall>::Provisions>>::Output,
    <R as TAppend<<Pl as PluginInstall>::Required>>::Output,
    <<Pl as PluginInstall>::Controllers as PushPluginCtrls<Pl, Mods>>::Output,
>;

/// Provision list after installing the plugins a module
/// [brings](FeatureModule::Plugins) — the `P` every later step of
/// `register_module` builds on.
pub type ModulePluginsP<M, P, R, Mods> =
    <<M as FeatureModule>::Plugins as ModulePlugins<P, R, Mods>>::OutP;

/// Requirement list after installing a module's brought plugins.
pub type ModulePluginsR<M, P, R, Mods> =
    <<M as FeatureModule>::Plugins as ModulePlugins<P, R, Mods>>::OutR;

/// Deferred-controller list after installing a module's brought plugins.
pub type ModulePluginsMods<M, P, R, Mods> =
    <<M as FeatureModule>::Plugins as ModulePlugins<P, R, Mods>>::OutMods;

/// Builder returned by
/// [`register_module`](registration::RegisterModule::register_module): the
/// plugins the module [brings](FeatureModule::Plugins) are installed first
/// (growing `P`/`R`/`Mods` exactly as `.plugin(..)` would), then the module's
/// `Exports` join the provision list `P`, its `Imports` join the requirement
/// list `R`, and the module is queued on `Mods` so `build_state()` registers
/// its controllers.
pub type ModuleRegistered<M, P, R, Mods> = AppBuilder<
    NoState,
    ModuleRegisteredP<M, P, R, Mods>,
    ModuleRegisteredR<M, P, R, Mods>,
    ModuleRegisteredMods<M, P, R, Mods>,
>;

/// The provision list [`ModuleRegistered`] carries — its brought plugins'
/// provisions plus the module's `Exports`. Split out of the builder alias so
/// the [`ModuleGroup`] fold can thread it through the next member.
pub type ModuleRegisteredP<M, P, R, Mods> =
    <<M as FeatureModule>::Exports as TAppend<ModulePluginsP<M, P, R, Mods>>>::Output;

/// The requirement list [`ModuleRegistered`] carries.
pub type ModuleRegisteredR<M, P, R, Mods> =
    <ModulePluginsR<M, P, R, Mods> as TAppend<<M as FeatureModule>::Imports>>::Output;

/// The pending-module list [`ModuleRegistered`] carries.
pub type ModuleRegisteredMods<M, P, R, Mods> =
    TCons<ModEntry<M>, ModulePluginsMods<M, P, R, Mods>>;

type ConsumerReg<T> =
    Box<dyn FnOnce(T) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>;

/// A queued controller-core `#[post_construct]` future, awaited at startup
/// before consumer registrations. State-free (the future already captures the
/// core `Arc`), so — unlike [`ConsumerReg`] — it carries no `T`.
type PostConstructReg = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>
            + Send,
    >,
>;

/// A queued `#[on_start]` hook with its declared `order`, collected from beans
/// (via [`BeanRegistry::register_on_start`](crate::beans::BeanRegistry::register_on_start))
/// and from controller cores (via [`Controller::on_start`](crate::Controller::on_start)).
/// Sorted by `order` (ascending, ties in registration order) and awaited in
/// sequence at startup, before the builder's [`StartupHook`] closures.
type OnStartReg = (i32, crate::beans::OnStartHook);

type LayerFn = Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>;

/// A meta consumer that drains typed metadata from the registry and returns
/// a router fragment to be merged into the application.
type MetaConsumer<T> = Box<dyn FnOnce(&MetaRegistry) -> crate::http::Router<T> + Send>;

/// A serve hook, called when the server starts. Receives a [`ServeContext`]
/// tying the hook into the app's shutdown sequence.
///
/// `each_cycle` marks a hook registered through
/// [`DeferredContext::on_serve_each_cycle`](crate::plugin::DeferredContext::on_serve_each_cycle):
/// it also runs on `r2e dev` hot-patch cycles, which skip the rest of the
/// startup lifecycle (see `PreparedApp::start_lifecycle`).
#[doc(hidden)]
pub struct ServeHook {
    pub(crate) hook: Box<dyn FnOnce(ServeContext) + Send>,
    pub(crate) each_cycle: bool,
}

/// A listener handed out by [`ServeContext::bind_tcp`].
///
/// `handover` is cancelled when a later dev-reload cycle takes the same
/// socket (never in production). Serve through [`into_incoming`]: the
/// stream checks the handover before every accept and, when it ends (or is
/// dropped) — or when [`stop_signal`] resolves — tells the store this holder
/// has released the socket. The next cycle's `bind_tcp` waits for that
/// acknowledgement before it gets the socket (bounded: a holder that has
/// not acknowledged after 5 s is logged and overridden), so barring that
/// fail-open no connection is accepted by the previous server once the new
/// one is serving.
///
/// [`stop_signal`]: BoundListener::stop_signal
///
/// [`into_incoming`]: BoundListener::into_incoming
pub struct BoundListener {
    /// The bound socket.
    pub listener: crate::rt::TcpListener,
    /// Fires when a later cycle takes over the socket.
    pub handover: CancelToken,
    /// Cancelled on drop: "this holder no longer accepts".
    pub(crate) release: ReleaseGuard,
}

/// Fires its token on drop — the holder's acknowledgement that it stopped
/// accepting (whether it exited cleanly, was dropped, or never served).
pub(crate) struct ReleaseGuard(pub(crate) CancelToken);

impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl BoundListener {
    /// A future that resolves when either the given shutdown token or the
    /// handover token fires — the signal to stop accepting connections.
    ///
    /// Its resolution **is** the holder's acknowledgement: the moment it
    /// resolves the socket is released to the next dev-reload holder, so the
    /// caller must not accept after awaiting it. This is what a
    /// `select!`-style accept loop (tonic's) needs — it may take the signal
    /// branch, break, and keep the incoming stream alive while draining
    /// connections; the release must not wait for that drain.
    pub fn stop_signal(
        &self,
        shutdown: CancelToken,
    ) -> impl std::future::Future<Output = ()> + Send + 'static {
        let handover = self.handover.clone();
        let released = self.release.0.clone();
        async move {
            crate::rt::select! {
                _ = shutdown.cancelled() => {}
                _ = handover.cancelled() => {}
            }
            released.cancel();
        }
    }

    /// Turn the listener into an accept stream that ends on `shutdown` or on
    /// the handover, checking those *before* every accept — so a queued
    /// connection is never taken once the stop was signalled — and releases
    /// the socket to the next holder when it ends or is dropped. Feed it to
    /// `serve_with_incoming_shutdown` (tonic) together with
    /// [`stop_signal`](Self::stop_signal), or drive it from any accept loop.
    pub fn into_incoming(self, shutdown: CancelToken) -> HandoverIncoming {
        let stop = Box::pin(self.stop_signal(shutdown));
        HandoverIncoming {
            listener: self.listener,
            stop,
            release: Some(self.release),
            done: false,
        }
    }
}

/// Accept stream of a [`BoundListener`] — see [`BoundListener::into_incoming`].
///
/// Yields `Result<TcpStream, io::Error>`; ends (`None`) once the stop signal
/// fired. Ending, or dropping the stream, releases the socket to the next
/// dev-reload holder.
pub struct HandoverIncoming {
    listener: crate::rt::TcpListener,
    stop: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
    release: Option<ReleaseGuard>,
    done: bool,
}

impl HandoverIncoming {
    /// Local address of the underlying socket.
    pub fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.listener.local_addr()
    }
}

impl futures_core::Stream for HandoverIncoming {
    type Item = Result<crate::rt::TcpStream, std::io::Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;
        if self.done {
            return Poll::Ready(None);
        }
        // Stop signal first, then accept: never take a connection once the
        // stop (shutdown or handover) is observable.
        if self.stop.as_mut().poll(cx).is_ready() {
            self.done = true;
            // Acknowledge: the next holder may start serving now.
            drop(self.release.take());
            return Poll::Ready(None);
        }
        match self.listener.poll_accept(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok((stream, _))) => Poll::Ready(Some(Ok(stream))),
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
        }
    }
}

/// Context handed to serve hooks ([`DeferredContext::on_serve`]) when the
/// server starts.
///
/// Ties serve-time subsystems into the app's lifecycle:
/// - [`task_registry`](Self::task_registry) — the shared task registry; each
///   hook drains the tasks it owns (`take_of::<Tag>()` for tagged tasks, or
///   `take_all()` for single-consumer subsystems).
/// - [`shutdown_token`](Self::shutdown_token) — cancelled when graceful
///   shutdown begins (after drain hooks), while in-flight HTTP requests are
///   still finishing. Use it to stop accepting new work.
/// - [`track`](Self::track) / [`track_named`](Self::track_named) — spawn a task
///   whose completion is awaited after the HTTP drain, before user shutdown
///   hooks (bounded **per handle** by
///   [`AppBuilder::shutdown_grace_period`]). Track any server-like task that
///   drains on the shutdown token so the process doesn't exit mid-drain.
///   Prefer `track_named`: the label is what the grace-period warning names
///   when that task is the one holding shutdown up.
pub struct ServeContext {
    tasks: TaskRegistryHandle,
    shutdown: CancelToken,
    handles: ServiceHandles,
    /// The resolved bean graph, cloned into every task `track` spawns — see
    /// [`ServeContext::track`].
    graph: Arc<crate::beans::BeanContext>,
}

impl ServeContext {
    /// The shared task registry (scheduled tasks, tagged subsystem tasks).
    pub fn task_registry(&self) -> TaskRegistryHandle {
        self.tasks.clone()
    }

    /// Token cancelled when graceful shutdown begins.
    pub fn shutdown_token(&self) -> crate::rt::CancelToken {
        self.shutdown.clone()
    }

    /// Bind the TCP listener a serve hook serves its own port from.
    ///
    /// In production this is a plain (async) bind and the returned
    /// [`BoundListener::handover`] token never fires. Inside the `r2e dev`
    /// hot-patch loop it goes through the same process-global listener store
    /// the HTTP port uses, namespaced by `owner` (so a gRPC transport and the
    /// HTTP server asking for the same address string get *different*
    /// sockets): the socket is bound once and every later cycle receives a
    /// clone of it, so the port stays open — and stays the *same* port, even
    /// for `:0` — across patches. Taking the socket cancels the handover
    /// token handed to the previous cycle and then **waits for that holder
    /// to acknowledge** (its [`HandoverIncoming`] ending or being dropped,
    /// or its [`BoundListener::stop_signal`] resolving) before returning:
    /// the previous server has stopped accepting before the new cycle's
    /// server starts, so no queued connection is answered with stale routes.
    /// The wait is bounded (5 s): a holder that never acknowledges is logged
    /// and overridden — fail-open, so a stuck task cannot wedge the dev
    /// loop; only then can the old server still take a queued connection.
    /// Serve through [`BoundListener::into_incoming`] to take part in that
    /// protocol.
    ///
    /// Pair it with
    /// [`on_serve_each_cycle`](crate::plugin::DeferredContext::on_serve_each_cycle):
    /// a hook that only runs once cannot re-serve the port after a patch.
    /// The future borrows nothing from the context, so a hook can move it
    /// into the task it tracks.
    pub fn bind_tcp(
        &self,
        owner: &'static str,
        addr: &str,
    ) -> impl std::future::Future<Output = Result<BoundListener, crate::beans::BootError>> + Send + 'static
    {
        let addr = addr.to_string();
        async move {
            #[cfg(feature = "dev-reload")]
            if crate::runtime::dev::hot_reload_loop_active() {
                return crate::runtime::dev::bind_listener(owner, addr).await;
            }
            #[cfg(not(feature = "dev-reload"))]
            let _ = owner;
            Ok(BoundListener {
                listener: crate::rt::bind_tcp(&addr).await?,
                handover: CancelToken::new(),
                release: ReleaseGuard(CancelToken::new()),
            })
        }
    }

    /// Spawn a tracked task: its completion is awaited after the HTTP drain
    /// completes, before user shutdown hooks.
    ///
    /// **Every task a serve hook starts belongs here** — a bare `rt::spawn` is
    /// neither cancelled, nor awaited, nor graph-owning.
    ///
    /// The task is expected to stop when
    /// [`shutdown_token`](Self::shutdown_token) fires: `run()` cancels it and
    /// then joins the tracked handles on every exit it controls — the normal
    /// shutdown *and* an aborted boot (a startup hook returning `Err` after
    /// this hook ran, a serve error) — bounded by `shutdown_grace_period` when
    /// one is configured.
    ///
    /// Takes the **future**, not a `JobHandle`, on purpose: the task is
    /// wrapped so it owns a strong reference to the bean graph for its whole
    /// lifetime, which makes `GraphHandle` resolution inside it sound even on
    /// the exits where nothing joins it (the grace period elapsing, or the
    /// whole `run()` future being dropped by an `r2e dev` hot patch). A
    /// pre-spawned handle could not be given that ownership after the fact.
    ///
    /// Prefer [`track_named`](Self::track_named): an unnamed task that eats the
    /// grace period is reported as `<unnamed>` and tells the operator nothing.
    pub fn track<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.track_named(UNNAMED_TRACKED_TASK, fut);
    }

    /// [`track`](Self::track), with a label used in shutdown diagnostics.
    ///
    /// `name` identifies the task in the `shutdown_grace_period` warning that
    /// fires when *this* handle is the one that did not finish in time —
    /// without it the operator only learns that "a background task" hung.
    /// Use a stable, human-readable name (`"grpc server"`,
    /// `"scheduler driver"`, …).
    pub fn track_named<F>(&self, name: &'static str, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.handles
            .spawn_owning(name, Arc::clone(&self.graph), fut);
    }
}

/// Label used for tracked tasks registered through the unnamed
/// [`ServeContext::track`].
pub(super) const UNNAMED_TRACKED_TASK: &str = "<unnamed>";

/// A tracked task handle plus the name shutdown diagnostics report it under.
pub(super) struct TrackedHandle {
    /// Human-readable name of the task, from `track_named` /
    /// `spawn_service`'s component type name. [`UNNAMED_TRACKED_TASK`] when
    /// the caller did not give one.
    pub(super) label: &'static str,
    pub(super) handle: crate::rt::JobHandle<()>,
}

/// Shared collection of JobHandles awaited after the HTTP drain: services
/// spawned via [`AppBuilder::spawn_service`] and serve-hook tasks registered
/// through [`ServeContext::track`]. Shutdown awaits their completion, each
/// handle bounded on its own by `shutdown_grace_period`, before returning.
#[derive(Clone, Default)]
struct ServiceHandles(Arc<Mutex<Vec<TrackedHandle>>>);

impl ServiceHandles {
    /// Spawn `fut` as a tracked task that **owns the bean graph while it
    /// runs**, under the diagnostic name `label`.
    ///
    /// The single constructor for tracked work (`ServeContext::track`,
    /// `spawn_service`, the QUIC endpoint drain), so the ownership rule holds
    /// by construction: awaiting the handle is best-effort — abandoned when
    /// `shutdown_grace_period` elapses, and skipped entirely when the `run()`
    /// future itself is dropped — but the graph reference travels *inside* the
    /// task and is released only when the task itself ends.
    fn spawn_owning<F>(&self, label: &'static str, graph: Arc<crate::beans::BeanContext>, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.push(TrackedHandle {
            label,
            handle: crate::rt::spawn(async move {
                // Named binding: lives until the end of this block, i.e. until
                // the wrapped future has completed.
                let _graph_keepalive = graph;
                fut.await;
            }),
        });
    }

    /// [`spawn_owning`](Self::spawn_owning) on the **control plane**.
    ///
    /// Same ownership rule; the difference is which runtime the task lands on.
    /// Used for work started from inside a request handler — today, upgraded
    /// WebSocket sessions ([`WsSessions`]). Under SO_REUSEPORT sharded serving
    /// a handler runs on a worker's `current_thread` runtime, torn down when
    /// that worker leaves its serve loop, i.e. *before* these handles are
    /// joined; `spawn_ctl` puts the task on the main runtime, which outlives
    /// the workers. On the single-listener path it is a plain `spawn`.
    #[cfg(feature = "ws")]
    fn spawn_owning_ctl<F>(
        &self,
        label: &'static str,
        graph: Arc<crate::beans::BeanContext>,
        fut: F,
    ) where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.push(TrackedHandle {
            label,
            handle: crate::rt::spawn_ctl(async move {
                let _graph_keepalive = graph;
                fut.await;
            }),
        });
    }

    fn push(&self, handle: TrackedHandle) {
        let mut handles = self.0.lock().unwrap();
        // Drop what has already finished. Services and serve-hook tasks are
        // registered once at boot, but WebSocket sessions push one handle per
        // connection: without this the vector would grow for the lifetime of
        // the process on a long-running server.
        handles.retain(|h| !h.handle.is_finished());
        handles.push(handle);
    }

    fn drain(&self) -> Vec<TrackedHandle> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }

    /// Whether any tracked task is still running.
    ///
    /// What [`RunningApp::has_shutdown_work`](crate::RunningApp::has_shutdown_work)
    /// asks: a task on this lane is joined (under `shutdown_grace_period`) by
    /// the shutdown sequence, so "there is one" means "shutdown has work".
    /// Finished handles do not count — `push` already prunes them, and a
    /// service that ended on its own needs no teardown.
    fn has_live(&self) -> bool {
        self.0
            .lock()
            .unwrap()
            .iter()
            .any(|h| !h.handle.is_finished())
    }

    /// Abort every tracked task and clear the lane, returning how many were
    /// still running.
    ///
    /// The teardown of last resort, for the paths that cannot await a join:
    /// [`RunningApp`](crate::RunningApp) dropped without `shutdown()`. These
    /// handles are never joined afterwards — nothing else holds them — so
    /// abort is the honest semantics: dropping the handle alone would *detach*
    /// the task, leaving it running against a graph the test believes is gone.
    fn abort_all(&self) -> usize {
        let handles = self.drain();
        let mut aborted = 0;
        for h in handles {
            if !h.handle.is_finished() {
                aborted += 1;
                h.handle.abort();
            }
        }
        aborted
    }
}

/// The app-scope shutdown token, created **lazily** by whichever comes first
/// (a `register_service` call or `run_inner`), memoized in
/// `plugin_data` so `run_inner` and everything registered before it agree on
/// one root.
///
/// Serving cancels this token on every exit — including the uncontrolled ones,
/// where a `DropGuard` fires it (a panic unwinding out of `run_inner`, or the
/// whole `run()` future being dropped by an `r2e dev` hot patch). That is only
/// worth anything if the token a task waits on is *reachable* from here, which
/// is why [`AppBuilder::register_service`] mints its per-service token as a
/// `child_token()` of this one instead of a fresh, disconnected token: a child
/// can be cancelled on its own (the plugin sync shutdown hooks still do that,
/// early in the shutdown sequence) **and** is cancelled with its parent.
#[derive(Clone)]
struct ShutdownRoot(CancelToken);

/// The provision list every [`AppBuilder::new`] starts from.
///
/// R2E provides exactly one bean before the app declares anything: the
/// [`ShutdownToken`](crate::rt::ShutdownToken). It is a **normal** bean — it
/// grows the compile-time provision list `P` like a hand-written
/// `.provide(...)`, it lands in the state HList, and a `#[module]` must list it
/// in `imports(...)` to inject it (there are no ambient beans). The only thing
/// special about it is that the builder writes the `.provide` for you, because
/// only the builder can mint a token on the app's shutdown lineage.
///
/// `WsSessions` is deliberately **not** here: generated `#[ws]` code reads it
/// from the bean context at registration, never from the state, so putting it
/// on `P` would only widen every app's state for nobody's benefit.
pub type BuiltinProvisions = TCons<crate::rt::ShutdownToken, TNil>;

/// Get-or-insert the one [`ShutdownRoot`] for this app.
///
/// Called from `register_service` (build time, first writer) and from
/// `run_inner` (serve time), which is why it is a free function over the
/// `plugin_data` map both phases own.
fn shutdown_root(data: &mut HashMap<TypeId, Box<dyn Any + Send + Sync>>) -> CancelToken {
    data.entry(TypeId::of::<ShutdownRoot>())
        .or_insert_with(|| Box::new(ShutdownRoot(CancelToken::new())))
        .downcast_ref::<ShutdownRoot>()
        .expect("ShutdownRoot type mismatch in plugin_data")
        .0
        .clone()
}

/// Resolve the active profile: forced (`with_profile`) > `R2E_PROFILE` env >
/// `r2e.profile` config > `"default"`.
fn resolve_profile(forced: Option<&str>, config: &crate::config::R2eConfig) -> String {
    forced
        .map(str::to_string)
        .or_else(|| std::env::var("R2E_PROFILE").ok())
        .or_else(|| config.try_get::<String>("r2e.profile"))
        .unwrap_or_else(|| "default".to_string())
}

/// The [`LiveConfigRegistry`](crate::config::LiveConfigRegistry) this
/// `load_config` cycle must hand to the bean graph.
///
/// Outside the Subsecond hot-patch loop — production, tests, anything that
/// never calls `dev::mark_hot_reload_loop()` — this is a plain
/// `LiveConfigRegistry::from_config`, one fresh registry per `load_config`.
///
/// **Inside** the loop the registry has a single identity for the whole
/// process: `#[live_config]` handles bind one slot of one registry forever, so
/// the surviving instance is re-seeded from the freshly loaded config rather
/// than replaced (see [`LiveConfigRegistry::reseed`] for the diff rules).
fn live_config_registry_for_cycle(
    config: &crate::config::R2eConfig,
    pinned_keys: std::collections::HashSet<String>,
) -> crate::config::LiveConfigRegistry {
    #[cfg(feature = "dev-reload")]
    {
        if let Some(carried) = crate::runtime::dev::carried_live_config_registry() {
            carried.reseed(config, pinned_keys);
            return carried;
        }
        let fresh = crate::config::LiveConfigRegistry::from_config(config, pinned_keys);
        crate::runtime::dev::carry_live_config_registry(&fresh);
        return fresh;
    }
    #[cfg(not(feature = "dev-reload"))]
    crate::config::LiveConfigRegistry::from_config(config, pinned_keys)
}

/// Marker type: application state has not been set yet.
///
/// `AppBuilder<NoState>` is the initial phase returned by [`AppBuilder::new()`].
/// Call [`.with_state()`](AppBuilder::with_state) or [`.build_state()`](AppBuilder::build_state)
/// to transition to `AppBuilder<T>`.
#[derive(Clone)]
pub struct NoState;

/// Shared configuration that is independent of the application state type.
struct BuilderConfig {
    config: Option<crate::config::R2eConfig>,
    custom_layers: Vec<LayerFn>,
    /// Transport-level router transforms, applied OUTERMOST — after
    /// `custom_layers` and `catch_panic_layer`. The wrapped service sees
    /// every request before any HTTP middleware; the inner HTTP router keeps
    /// its full middleware stack. Used by transport multiplexers (e.g. gRPC
    /// content-type routing) so non-HTTP traffic never crosses HTTP-shaped
    /// layers.
    router_wraps: Vec<LayerFn>,
    bean_registry: BeanRegistry,
    /// Deferred actions to be executed after state resolution.
    deferred_actions: Vec<DeferredAction>,
    /// Plugin data storage (type-erased, keyed by TypeId).
    plugin_data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    /// Routes-stage plugin effects: queued by the Graph-stage deferred action
    /// and drained in `build()`, once every controller has registered.
    routes_effects: Vec<RoutesEffect>,
    /// Whether to install the pre-routing trailing-slash normalization rewrite.
    normalize_path: bool,
    /// Application callback invoked once per caught panic, set via
    /// [`AppBuilder::on_panic`]. Handed to both catch-panic install slots.
    panic_hook: Option<crate::runtime::panic::PanicHook>,
    /// Whether the DevReload plugin has been applied (prevents double-install).
    dev_reload_applied: bool,
    /// Maximum time allowed for the tracked-handle join phase, applied to each
    /// handle on its own. `None` means wait indefinitely (default).
    shutdown_grace_period: Option<Duration>,
    /// Explicit HTTP-drain budget set on the builder, as a two-level option:
    /// the outer `None` means "not set — resolve from `server.drain-timeout`
    /// or the 30s default"; `Some(Some(d))` is
    /// [`AppBuilder::drain_timeout`]; `Some(None)` is the deliberate
    /// [`AppBuilder::drain_timeout_unbounded`] opt-out. Resolved once in
    /// `build_inner` via
    /// [`resolve_drain_timeout`](crate::runtime::drain::resolve_drain_timeout).
    drain_timeout: Option<Option<Duration>>,
    /// Active profile name, resolved from the forced profile
    /// ([`AppBuilder::with_profile`]), `R2E_PROFILE` env var, `r2e.profile`
    /// config key, or `"default"`.
    active_profile: String,
    /// Profile forced via [`AppBuilder::with_profile`]; wins over env/config
    /// detection. Set by test harnesses (no process-global env mutation).
    forced_profile: Option<String>,
    /// Base config file set via [`AppBuilder::with_config_file`]; used by
    /// `load_config` instead of the default `application.yaml`.
    config_file: Option<std::path::PathBuf>,
    /// Config keys stashed via [`AppBuilder::override_config_value`] before
    /// config is loaded; applied on top of whatever `load_config` produces
    /// (always drained *after* the base config, so they win).
    config_overrides: Vec<(String, crate::config::ConfigValue)>,
    /// Pre-loaded config stashed via [`AppBuilder::override_config`]; consumed
    /// by `load_config` in place of the disk read. Test harnesses and the
    /// dev-reload loop set it. Left `Some` at `build_state` (never consumed by
    /// a `load_config` call) is a panic — the config would be silently ignored.
    preloaded_config: Option<crate::config::R2eConfig>,
    /// External config providers applied by `load_config`.
    config_providers: Vec<Arc<dyn crate::config::ConfigProvider>>,
    /// The live-config registry created by `load_config` (the very same
    /// `Arc`-shared instance handed to the bean registry and to provider watch
    /// tasks). Retained so a *late*
    /// [`AppBuilder::override_config_value`](AppBuilder::override_config_value)
    /// can patch and pin the live slot too, not just `R2eConfig`.
    live_config: Option<crate::config::LiveConfigRegistry>,
    /// Stop handle wired via [`AppBuilder::with_stop_handle`]; `prepare()`
    /// creates one lazily when absent.
    stop_handle: Option<StopHandle>,
    /// Pre-destroy disposers, drained from the resolved [`BeanContext`] at
    /// `build_state()` and folded into the async shutdown phase by
    /// [`AppBuilder::from_pre`].
    bean_disposers: Vec<crate::plugin::AsyncShutdownHook>,
    /// Per-worker service factories registered via
    /// [`AppBuilder::per_worker_service`]; run once inside every sharded
    /// worker before it serves (see [`crate::runtime::worker`]).
    per_worker_services: Vec<crate::runtime::worker::PerWorkerServiceFactory>,
    /// First boot failure recorded by a builder step that cannot return a
    /// `Result` — today only `load_config()`, a type-state transition in the
    /// middle of the chain. `try_build_state()` surfaces it before any bean is
    /// constructed, so a bad config file reaches `app_main!`'s exit-1 contract
    /// and `TestApp::try_boot` instead of panicking.
    deferred_boot_error: Option<crate::beans::BeanError>,
}

impl BuilderConfig {
    /// Record a boot failure raised by an infallible-by-signature builder
    /// step. The **first** failure wins: later steps run against a degraded
    /// (empty) config and their follow-on errors would only bury the cause.
    fn record_boot_error(&mut self, err: crate::beans::BeanError) {
        if self.deferred_boot_error.is_none() {
            self.deferred_boot_error = Some(err);
        }
    }
}

/// Builder for assembling a R2E application.
///
/// Collects state, controller routes, and Tower layers, then produces an
/// `r2e::http::Router` (or starts serving directly) with everything wired together.
///
/// # Two-phase builder
///
/// The builder starts in the `NoState` phase (`AppBuilder<NoState>`), where
/// you can call [`provide()`](Self::provide), [`register()`](Self::register),
/// and state-independent configuration methods. Transition to a typed phase
/// via:
///
/// - [`.with_state(state)`](AppBuilder::<NoState>::with_state) — provide a pre-built state directly.
/// - [`.build_state()`](AppBuilder::<NoState>::build_state) — resolve the bean graph and build state.
///
/// Plugins install with [`.plugin(p)`](AppBuilder::plugin) in the builder
/// phase, *before* the transition: their `build` runs as a graph node inside
/// `build_state()`. Once in the typed phase (`AppBuilder<T>`), you register
/// controllers, add hooks, and call `.build()` or `.serve()`.
pub struct AppBuilder<
    T: Clone + Send + Sync + 'static = NoState,
    P = BuiltinProvisions,
    R = TNil,
    Mods = TNil,
> {
    shared: BuilderConfig,
    state: T,
    /// The resolved bean graph, retained through the typed phase so controller
    /// cores (and background services) can be constructed by type via
    /// `ctx.get::<T>()`. An empty placeholder before `build_state()` and on the
    /// `with_state` path.
    bean_context: Arc<crate::beans::BeanContext>,
    routes: Vec<crate::http::Router<T>>,
    startup_hooks: Vec<StartupHook<T>>,
    shutdown_hooks: Vec<ShutdownHook<T>>,
    drain_hooks: Vec<DrainHook<T>>,
    meta_registry: MetaRegistry,
    meta_consumers: Vec<MetaConsumer<T>>,
    consumer_registrations: Vec<ConsumerReg<T>>,
    /// Controller-core `#[post_construct]` futures, awaited at startup before
    /// `consumer_registrations`.
    post_construct_registrations: Vec<PostConstructReg>,
    /// `#[on_start]` hooks from beans and controller cores, awaited at startup
    /// (sorted by declared order) after the consumer registrations and before
    /// the plugin serve hooks and the builder's `on_start` closures.
    on_start_hooks: Vec<OnStartReg>,
    /// Serve hooks from plugins (called when server starts).
    /// Tasks already capture their state, so only the token is needed.
    serve_hooks: Vec<ServeHook>,
    /// Shutdown hooks from plugins (sync).
    plugin_shutdown_hooks: Vec<Box<dyn FnOnce() + Send>>,
    /// Shutdown hooks from plugins (async, awaited during shutdown).
    plugin_async_shutdown_hooks: Vec<crate::plugin::AsyncShutdownHook>,
    /// Controller-core `#[pre_destroy]` disposal hooks, pushed in registration
    /// order as controllers register and folded into the ordered async-shutdown
    /// list at `build_inner` (reversed there so later-registered controllers
    /// dispose first), after the plugin async hooks and before the bean disposers.
    controller_disposers: Vec<crate::plugin::AsyncShutdownHook>,
    /// Bean `#[pre_destroy]` disposers, drained from the resolved graph at
    /// `build_state()` (reverse registration order) and run at the end of the
    /// async shutdown phase.
    bean_disposers: Vec<crate::plugin::AsyncShutdownHook>,
    _provided: PhantomData<P>,
    _required: PhantomData<R>,
    /// Pending feature modules whose controllers `build_state()` registers.
    _modules: PhantomData<Mods>,
}

// ── Removed post-state plugin API (migration diagnostics) ───────────────────

mod post_state_removed {
    /// Sealed: outside this module nothing can name — let alone implement —
    /// this trait, which is the point.
    pub trait Sealed {}
}

/// Marker for the **removed** post-state plugin API.
///
/// Nothing implements it (its supertrait is sealed and unimplemented), so the
/// only thing it can do is turn a leftover
/// [`AppBuilder::with`](AppBuilder::with) call into a compile error that names
/// the migration. See `docs/migration/plugin-api.md`.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`AppBuilder::with` was removed: install `{Self}` with `.plugin(..)` BEFORE `build_state()` (see docs/migration/plugin-api.md)",
    label = "the post-state plugin API is gone",
    note = "there is only one plugin kind now: implement `r2e::Plugin` for `{Self}` (an async `build(self, deps, config, ctx)`) and install it with `.plugin({Self})` before `build_state()`",
    note = "post-state effects moved onto `PluginBuildContext`: `add_layer` (Graph), `after_routes` (Routes), `wrap_router` (Finalize, the replacement for `should_be_last()`)"
)]
pub trait PostStatePluginRemoved: post_state_removed::Sealed {}

// ── Conditional assembly (any phase) ────────────────────────────────────────

impl<T: Clone + Send + Sync + 'static, P, R, Mods> AppBuilder<T, P, R, Mods> {
    /// **Removed.** The post-state plugin API (`.with(plugin)` after
    /// `build_state()`) no longer exists; there is one plugin kind, installed
    /// with [`.plugin(..)`](AppBuilder::plugin) *before* `build_state()`.
    ///
    /// This shim exists only so the leftover call fails at **compile time**
    /// with a message that names the migration instead of a bare "no method
    /// `with`". Its bound, [`PostStatePluginRemoved`], is implemented by
    /// nothing. See `docs/migration/plugin-api.md`.
    #[doc(hidden)]
    #[deprecated(
        note = "`AppBuilder::with` was removed with the post-state plugin API: implement `Plugin` and install it with `.plugin(..)` BEFORE `build_state()` — see docs/migration/plugin-api.md"
    )]
    pub fn with<Pl: PostStatePluginRemoved>(self, _plugin: Pl) -> Self {
        self
    }
    /// Returns a reference to the loaded [`R2eConfig`](crate::config::R2eConfig),
    /// if any.
    ///
    /// Available after `load_config` (or `with_config`). Plugins install **before**
    /// `build_state()`, so this is the accessor a config-driven plugin
    /// constructor reads from:
    ///
    /// ```ignore
    /// let b = AppBuilder::new().load_config::<AppConfig>();
    /// let tracing = Tracing::from_config(b.r2e_config().unwrap());
    /// b.plugin(tracing).build_state().await
    /// ```
    pub fn r2e_config(&self) -> Option<&crate::config::R2eConfig> {
        self.shared.config.as_ref()
    }

    /// Conditionally apply a builder transformation.
    ///
    /// `f` must return the **same** builder type, so it may call `Self -> Self`
    /// methods (custom layers, plugins, config toggles) but **not** type-changing
    /// methods like `register`: a runtime flag cannot change the compile-time
    /// provision list `P`. For conditional *bean* presence, use a
    /// `#[producer] -> Option<T>` — the slot is always in `P` and the producer
    /// decides `Some`/`None` internally.
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .when(cfg!(debug_assertions), |b| b.with_layer_fn(no_store))
    /// ```
    pub fn when(self, cond: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if cond {
            f(self)
        } else {
            self
        }
    }

    /// Register the application callback invoked once per **caught panic**.
    ///
    /// R2E already answers a panicking handler with a JSON 500 and logs one
    /// structured `error` event carrying the request span's `request_id` and
    /// `route`. This hook is the counting seam on top of that: R2E
    /// deliberately increments no metric of its own, because every service
    /// owns its registry and its metric prefix.
    ///
    /// ```ignore
    /// let panics = counter.clone();
    /// AppBuilder::new().on_panic(move |report| {
    ///     panics.with_label_values(&[report.route_label()]).inc();
    /// })
    /// ```
    ///
    /// The hook runs on the request task while the panic is being converted
    /// to a response, with the request span current. Keep it short and
    /// non-blocking. It cannot break the response: a panic inside the hook is
    /// caught, logged once, and the same JSON 500 still goes out. Calling
    /// `on_panic` twice replaces the previous hook.
    ///
    /// See [`PanicReport`](crate::PanicReport) for what it is told, and
    /// [`crate::runtime::panic`] for where the layer sits.
    pub fn on_panic<F>(mut self, hook: F) -> Self
    where
        F: Fn(&crate::runtime::panic::PanicReport<'_>) + Send + Sync + 'static,
    {
        self.shared.panic_hook = Some(Arc::new(hook));
        self
    }

    /// Wire a user-created [`StopHandle`] into the server lifecycle.
    ///
    /// Calling [`StopHandle::stop`] on (a clone of) the handle triggers the
    /// same graceful shutdown as an OS signal.
    ///
    /// Usually unnecessary: a `StopHandle` bean (`.provide(stop.clone())`,
    /// e.g. for an admin endpoint) is picked up automatically at
    /// [`prepare()`](AppBuilder::prepare), and without one
    /// [`PreparedApp::stop_handle`] hands back a fresh wired handle. Use this
    /// only to wire a handle that is neither a bean nor taken from the
    /// prepared app (it takes precedence over a bean).
    pub fn with_stop_handle(mut self, handle: StopHandle) -> Self {
        self.shared.stop_handle = Some(handle);
        self
    }

    /// Register a **per-worker service**: a shard-local, `!Send`-capable
    /// service constructed once inside every sharded HTTP worker
    /// (`server.workers`), after the worker runtime exists and before it
    /// accepts its first connection.
    ///
    /// `factory` is shared by all workers (`Send + Sync`) and invoked once per
    /// worker with that worker's [`WorkerContext`](crate::runtime::worker::WorkerContext)
    /// — worker id, worker count, shutdown token, and `spawn_local`. The future
    /// it returns and the [`WorkerService`](crate::runtime::worker::WorkerService)
    /// it resolves to run on, and never leave, the worker thread: `Rc`,
    /// `RefCell`, per-shard sockets (UDP `SO_REUSEPORT`, a QUIC endpoint) are
    /// all valid. Return `()` when there is nothing to clean up.
    ///
    /// Startup is all-or-nothing across workers (a failing factory fails
    /// `run()` with the worker id, after unwinding services already started);
    /// at graceful shutdown each worker drains HTTP, then awaits
    /// `WorkerService::shutdown` in reverse start order. Full guarantees in
    /// [`crate::runtime::worker`].
    ///
    /// Requires sharded serving: `run()` errors when a per-worker service is
    /// registered but `server.workers` is not set. Control-plane services
    /// (`spawn_service`, `#[scheduled]`, `#[consumer]`) are unaffected and
    /// remain the default for non-shard-local work.
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .per_worker_service(|worker| async move {
    ///         let sock = bind_reuseport_udp("0.0.0.0:4433")?; // std::net::UdpSocket
    ///         let sock = r2e::rt::UdpSocket::from_std(sock)?;  // adopted on this worker
    ///         Ok(ShardEcho::start(worker, sock))
    ///     })
    /// ```
    pub fn per_worker_service<F, Fut, S>(mut self, factory: F) -> Self
    where
        F: Fn(crate::runtime::worker::WorkerContext) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<S, crate::runtime::worker::BoxError>> + 'static,
        S: crate::runtime::worker::WorkerService,
    {
        self.shared
            .per_worker_services
            .push(crate::runtime::worker::PerWorkerServiceFactory::new(
                factory,
            ));
        self
    }
}

impl Default for AppBuilder<NoState, BuiltinProvisions, TNil, TNil> {
    fn default() -> Self {
        Self::new()
    }
}
