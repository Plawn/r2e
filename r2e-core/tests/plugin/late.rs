//! `GraphHandle` — the deferred-fill handle on the resolved graph — and
//! `Late<T>`, the underlying write-once cell (kept as an escape hatch).

use r2e_core::plugin::{GraphHandle, PluginBuildContext, PluginBuildError, PreStatePlugin};
use r2e_core::type_list::BeanAccess;
use r2e_core::{AppBuilder, Late};

use crate::fixtures::Alpha;

// ── GraphHandle ──────────────────────────────────────────────────────────────

/// A plugin-provided bean holding the `GraphHandle` its build captured, plus
/// what the handle looked like DURING build (must be empty — deps come
/// through `Deps`, the handle is for after-boot resolution).
#[derive(Clone)]
struct HandleHolder {
    handle: GraphHandle,
    empty_during_build: bool,
}

struct GraphCapturePlugin;

impl PreStatePlugin for GraphCapturePlugin {
    type Provided = (HandleHolder,);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(HandleHolder,), PluginBuildError> {
        let handle = ctx.graph();
        Ok((HandleHolder {
            empty_during_build: handle.get().is_none(),
            handle,
        },))
    }
}

#[r2e_core::test]
async fn graph_handle_fills_after_build_state() {
    let app = AppBuilder::new()
        .plugin(GraphCapturePlugin)
        .provide(Alpha(21))
        .build_state()
        .await;

    let holder = app.state().get::<HandleHolder>();
    // During build the graph did not exist yet…
    assert!(holder.empty_during_build, "handle is empty during build");
    // …after build_state the same handle sees the full resolved graph.
    assert!(holder.handle.get().is_some(), "handle filled after resolve");
    assert_eq!(holder.handle.bean::<Alpha>(), Some(Alpha(21)));
    // Missing beans resolve to None, not a panic.
    assert_eq!(holder.handle.bean::<String>(), None);
}

#[r2e_core::test]
async fn graph_is_released_once_the_app_is_dropped() {
    // The handle lives INSIDE the graph it points at (`BeanContext` →
    // `HandleHolder` → `GraphHandle`). A strong reference there is a cycle
    // that no drop can break — one leaked graph, with every pool and
    // connection in it, per boot (per hot-patch cycle under `r2e dev`).
    let app = AppBuilder::new()
        .plugin(GraphCapturePlugin)
        .provide(Alpha(21))
        .build_state()
        .await;

    let weak = std::sync::Arc::downgrade(app.bean_context());
    assert!(weak.upgrade().is_some(), "graph is alive while the app is");

    drop(app);
    assert!(
        weak.upgrade().is_none(),
        "the graph must be released with the app — a bean holding a \
         GraphHandle must not keep it alive"
    );
}

#[r2e_core::test]
async fn the_router_keeps_the_graph_alive() {
    // The other half of the weak handle: something outside the graph has to
    // own it, or beans would lose their graph the moment `build()` consumed
    // the builder — per-tenant maps and resource factories resolve through
    // the handle at REQUEST time, long after boot.
    let app = AppBuilder::new()
        .plugin(GraphCapturePlugin)
        .provide(Alpha(21))
        .build_state()
        .await;

    let weak = std::sync::Arc::downgrade(app.bean_context());
    let holder = app.state().get::<HandleHolder>();
    let router = app.build();

    // The builder is gone; the router is now the graph's only owner.
    assert!(weak.upgrade().is_some(), "router owns the graph");
    assert_eq!(
        holder.handle.bean::<Alpha>(),
        Some(Alpha(21)),
        "beans still resolve through the handle after build()"
    );

    drop(router);
    assert!(weak.upgrade().is_none(), "dropping the app drops the graph");
    assert_eq!(
        holder.handle.bean::<Alpha>(),
        None,
        "a handle outliving the graph reads empty, it does not panic"
    );
}

// ── The graph rides the request, not just the service ───────────────────────

/// A marker bean so the probe plugin has something to provide.
#[derive(Clone)]
struct ProbeMarker;

/// A plugin whose routes reach the graph through a weak `GraphHandle` at the
/// two moments the service value is already gone: after an `.await` inside the
/// handler, and from a response body that streams after the handler returned.
struct GraphProbePlugin;

impl PreStatePlugin for GraphProbePlugin {
    type Provided = (ProbeMarker,);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(ProbeMarker,), PluginBuildError> {
        let after_yield = ctx.graph();
        let streamed = ctx.graph();
        ctx.add_layer(move |router| {
            router
                .route(
                    "/after-yield",
                    r2e_core::http::routing::get(move || {
                        let handle = after_yield.clone();
                        async move {
                            // Yield first: with `oneshot`, the service (and any
                            // graph reference living only in it) is already gone
                            // by the time this future is first polled.
                            tokio::task::yield_now().await;
                            probe(&handle)
                        }
                    }),
                )
                .route(
                    "/streamed",
                    r2e_core::http::routing::get(move || {
                        let handle = streamed.clone();
                        async move {
                            let stream = futures_util::stream::unfold(0u8, move |step| {
                                let handle = handle.clone();
                                async move {
                                    match step {
                                        0 => {
                                            tokio::task::yield_now().await;
                                            Some((
                                                Ok::<_, std::io::Error>(bytes::Bytes::from_static(
                                                    b"",
                                                )),
                                                1u8,
                                            ))
                                        }
                                        // Resolved from the body, long after the
                                        // service future completed.
                                        1 => Some((Ok(bytes::Bytes::from(probe(&handle))), 2u8)),
                                        _ => None,
                                    }
                                }
                            });
                            r2e_core::http::Body::from_stream(stream)
                        }
                    }),
                )
        });
        Ok((ProbeMarker,))
    }
}

/// `"alive-<n>"` when the graph still resolves `Alpha`, `"gone"` when the weak
/// handle no longer upgrades.
fn probe(handle: &GraphHandle) -> String {
    match handle.bean::<Alpha>() {
        Some(Alpha(v)) => format!("alive-{v}"),
        None => "gone".to_string(),
    }
}

#[r2e_core::test]
async fn the_graph_outlives_a_request_future_whose_router_was_dropped() {
    // `ServiceExt::oneshot` replaces the service with its future the moment
    // `call` returns — BEFORE polling it. Owning the graph in the service value
    // alone therefore drops it under the running handler, and every
    // `GraphHandle` upgrade inside returns `None` (the tenant cascade turns that
    // into a `NoSource` 500). The `Arc` must ride the future itself.
    let app = AppBuilder::new()
        .plugin(GraphProbePlugin)
        .provide(Alpha(21))
        .build_state()
        .await;

    let weak = std::sync::Arc::downgrade(app.bean_context());
    let router = app.build();
    // The router is the graph's LAST owner and `send_get` moves it into
    // `oneshot`.
    let (status, body) = crate::support::send_get(router, "/after-yield").await;

    assert_eq!(status, r2e_core::http::StatusCode::OK);
    assert_eq!(body, "alive-21", "the graph survived into the handler");
    assert!(
        weak.upgrade().is_none(),
        "once the request is done and the router is gone, so is the graph"
    );
}

#[r2e_core::test]
async fn the_graph_outlives_a_streaming_response_body() {
    // Hyper splits the response into head and body and may drop the service —
    // and the head — while the body is still producing frames. Response
    // extensions are therefore NOT a sound anchor; the body itself has to carry
    // the graph.
    let app = AppBuilder::new()
        .plugin(GraphProbePlugin)
        .provide(Alpha(21))
        .build_state()
        .await;

    let weak = std::sync::Arc::downgrade(app.bean_context());
    let router = app.build();

    // The service future completes here, consuming the router…
    let response = crate::support::raw(
        router,
        "GET",
        "/streamed",
        &[],
        r2e_core::http::Body::empty(),
    )
    .await;
    assert_eq!(response.status(), r2e_core::http::StatusCode::OK);
    assert!(
        weak.upgrade().is_some(),
        "the body still holds the graph before it is collected"
    );

    // …and only now is the body drained.
    let body = crate::support::body_string(response).await;
    assert_eq!(body, "alive-21", "the graph survived into the body");
    assert!(
        weak.upgrade().is_none(),
        "collecting the body released the last reference"
    );
}

// ── Detached transports: the serving lifecycle owns the graph ───────────────

/// A plugin whose serve hook tracks a background task that touches the graph
/// **after** shutdown begins — the shape of a separate-port gRPC drain (or any
/// `spawn_service` task), without booting tonic.
struct TrackedDrainPlugin {
    observed: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

impl PreStatePlugin for TrackedDrainPlugin {
    type Provided = (ProbeMarker,);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(ProbeMarker,), PluginBuildError> {
        let handle = ctx.graph();
        let observed = self.observed;
        ctx.on_serve(move |sctx| {
            let token = sctx.shutdown_token();
            sctx.track(async move {
                token.cancelled().await;
                // The serve future completes (and with it the router, the last
                // router-side owner of the graph) as soon as the drain is done.
                // Tracked handles are awaited strictly after that, so by the
                // time this line runs the router is gone.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                *observed.lock().unwrap() = Some(probe(&handle));
            });
        });
        Ok((ProbeMarker,))
    }
}

#[tokio::test]
async fn a_tracked_drain_task_still_reaches_the_graph_after_the_router_is_gone() {
    // Tracked handles (`ServeContext::track`) are awaited AFTER the HTTP serve
    // future returns — separate-port gRPC drain, `spawn_service` tasks, the
    // QUIC endpoint drain. The router cannot own the graph on their behalf:
    // it is already dropped. The serving scope does.
    let observed = std::sync::Arc::new(std::sync::Mutex::new(None));
    let app = AppBuilder::new()
        .plugin(TrackedDrainPlugin {
            observed: observed.clone(),
        })
        .provide(Alpha(21))
        .build_state()
        .await;

    let weak = std::sync::Arc::downgrade(app.bean_context());
    let prepared = app.prepare("127.0.0.1:0");
    let stop = prepared.stop_handle();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server =
        r2e_core::rt::spawn(async move { prepared.run_with_listener(listener).await.is_ok() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    stop.stop();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .expect("server did not stop within 5s")
            .expect("server task panicked"),
        "run() returned an error"
    );

    assert_eq!(
        observed.lock().unwrap().as_deref(),
        Some("alive-21"),
        "a tracked task draining after the router was dropped must still \
         resolve beans through its GraphHandle"
    );
    assert!(
        weak.upgrade().is_none(),
        "and the graph is released once serving is over — the serve scope was \
         the owner, not something that leaked"
    );
}

/// A plugin whose serve hook tracks a task that is **never joined**: it blocks
/// on a gate the test opens only after `run()` has returned, so the
/// tracked-handle await cannot be what keeps its graph alive. It then probes
/// the graph and reports the result through `done`.
///
/// One plugin for both abnormal exits — an elapsed `shutdown_grace_period`
/// (join future dropped, task detached) and a startup hook returning `Err`
/// after the serve hooks already spawned (early `run_inner` return).
struct AbandonedTaskPlugin {
    gate: tokio::sync::oneshot::Receiver<()>,
    done: tokio::sync::oneshot::Sender<String>,
}

impl PreStatePlugin for AbandonedTaskPlugin {
    type Provided = (ProbeMarker,);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(ProbeMarker,), PluginBuildError> {
        let handle = ctx.graph();
        let AbandonedTaskPlugin { gate, done } = self;
        ctx.on_serve(move |sctx| {
            sctx.track(async move {
                let _ = gate.await;
                let _ = done.send(probe(&handle));
            });
        });
        Ok((ProbeMarker,))
    }
}

#[tokio::test]
async fn a_task_abandoned_by_the_grace_period_still_owns_its_graph() {
    // `shutdown_grace_period` bounds the tracked-handle await: when it elapses
    // the join futures are DROPPED and the tasks keep running, unwatched, past
    // the end of `run()`. Nothing outside them can own the graph on their
    // behalf — the task future has to.
    let (open_gate, gate) = tokio::sync::oneshot::channel();
    let (done, observed) = tokio::sync::oneshot::channel();
    let app = AppBuilder::new()
        .plugin(AbandonedTaskPlugin { gate, done })
        .provide(Alpha(21))
        .build_state()
        .await
        .shutdown_grace_period(std::time::Duration::from_millis(50));

    let weak = std::sync::Arc::downgrade(app.bean_context());
    let prepared = app.prepare("127.0.0.1:0");
    let stop = prepared.stop_handle();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server =
        r2e_core::rt::spawn(async move { prepared.run_with_listener(listener).await.is_ok() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    stop.stop();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .expect("the grace period must let run() return while the task is stuck")
            .expect("server task panicked"),
        "run() returned an error"
    );

    // `run()` has returned; the task is still blocked on the gate.
    assert!(
        weak.upgrade().is_some(),
        "the abandoned task must still own the graph after run() returned"
    );

    open_gate
        .send(())
        .expect("the tracked task is still waiting");
    let observed = tokio::time::timeout(std::time::Duration::from_secs(5), observed)
        .await
        .expect("the abandoned task did not report within 5s")
        .expect("the abandoned task was dropped instead of finishing");
    assert_eq!(
        observed, "alive-21",
        "a tracked task the grace period abandoned must still resolve beans \
         through its GraphHandle"
    );
}

/// A plugin whose serve hook tracks a task written the way the contract asks
/// for: it stops when the app's shutdown token fires, and nothing else. No
/// test-owned gate — the framework is the only thing that can end it.
struct TokenWaitingTaskPlugin {
    done: tokio::sync::oneshot::Sender<String>,
}

impl PreStatePlugin for TokenWaitingTaskPlugin {
    type Provided = (ProbeMarker,);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(ProbeMarker,), PluginBuildError> {
        let handle = ctx.graph();
        let done = self.done;
        ctx.on_serve(move |sctx| {
            let token = sctx.shutdown_token();
            sctx.track(async move {
                token.cancelled().await;
                let _ = done.send(probe(&handle));
            });
        });
        Ok((ProbeMarker,))
    }
}

#[tokio::test]
async fn a_startup_error_cancels_and_drains_the_work_serve_hooks_started() {
    // Serve hooks spawn tracked work BEFORE the fallible startup hooks run.
    // An `Err` there must not just return: the task is waiting on the shutdown
    // token (a separate-port gRPC listener does exactly this), so without a
    // cancel it listens forever, holding its port and — since tracked tasks own
    // the graph — the graph too. The abort path therefore cancels, then drains,
    // then returns the error.
    let (done, mut observed) = tokio::sync::oneshot::channel();
    let app = AppBuilder::new()
        .plugin(TokenWaitingTaskPlugin { done })
        .provide(Alpha(21))
        .build_state()
        .await
        .on_start(|_state| async {
            Err::<(), Box<dyn std::error::Error + Send + Sync>>("startup hook says no".into())
        });

    let weak = std::sync::Arc::downgrade(app.bean_context());
    let prepared = app.prepare("127.0.0.1:0");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let err = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        prepared.run_with_listener(listener),
    )
    .await
    .expect("run() must return: the aborted boot has to cancel the token it handed out")
    .expect_err("the failing startup hook must abort the boot");
    assert!(
        err.to_string().contains("startup hook says no"),
        "unexpected error: {err}"
    );

    // `try_recv`, not `await`: the value must ALREADY be there. That is the
    // drain — `run()` does not return until the tracked handles it can see have
    // been joined.
    let observed = observed
        .try_recv()
        .expect("the aborted boot must drain tracked tasks before returning");
    assert_eq!(
        observed, "alive-21",
        "a tracked task wound down by an aborted boot must still resolve beans \
         through its GraphHandle while it runs"
    );
    assert!(
        weak.upgrade().is_none(),
        "and once it has finished, nothing pins the graph"
    );
}

/// A plugin that hands its `GraphHandle` to the outside world and then fails
/// the boot — the one way to observe a handle that never gets filled.
struct FailingCapturePlugin {
    escaped: std::sync::Arc<std::sync::Mutex<Option<GraphHandle>>>,
}

impl PreStatePlugin for FailingCapturePlugin {
    type Provided = (Alpha,);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(Alpha,), PluginBuildError> {
        *self.escaped.lock().unwrap() = Some(ctx.graph());
        Err("backend unreachable".into())
    }
}

#[r2e_core::test]
async fn a_failed_boot_leaves_the_handle_empty_forever() {
    // The fill happens on the SUCCESSFUL exits of `try_build_state` only: a
    // failure returns before it. A handle that escaped a failed boot therefore
    // reads empty for good — documented, not a transient state to retry.
    let escaped = std::sync::Arc::new(std::sync::Mutex::new(None));
    let result = AppBuilder::new()
        .plugin(FailingCapturePlugin {
            escaped: escaped.clone(),
        })
        .try_build_state()
        .await;
    assert!(result.is_err(), "boot must fail");

    let handle = escaped.lock().unwrap().clone().expect("build captured it");
    assert!(handle.get().is_none(), "never filled");
    assert_eq!(handle.bean::<Alpha>(), None);
}

#[test]
fn default_graph_handle_is_empty() {
    let handle = GraphHandle::default();
    assert!(handle.get().is_none());
    assert_eq!(handle.bean::<Alpha>(), None);
}

// ── Late<T> (escape hatch) ───────────────────────────────────────────────────

#[test]
fn empty_cell_reads_none() {
    let cell: Late<String> = Late::new();
    assert!(cell.get().is_none());
}

#[test]
fn fill_then_get() {
    let cell = Late::new();
    cell.fill(42u32).unwrap();
    assert_eq!(cell.get(), Some(&42));
}

#[test]
fn first_fill_wins() {
    let cell = Late::new();
    cell.fill("first").unwrap();
    assert_eq!(cell.fill("second"), Err("second"));
    assert_eq!(cell.get(), Some(&"first"));
}

#[test]
fn clones_share_storage() {
    // Clones are handed out BEFORE the fill, and a fill through any handle
    // must be visible to all of them.
    let shell: Late<u32> = Late::new();
    let handed_out = shell.clone();
    assert!(handed_out.get().is_none());

    shell.fill(7).unwrap();
    assert_eq!(handed_out.get(), Some(&7));

    // Clones taken after the fill see it too.
    assert_eq!(shell.clone().get(), Some(&7));
}

#[test]
fn expect_returns_filled_value() {
    let cell = Late::new();
    cell.fill("ready").unwrap();
    assert_eq!(*cell.expect("my value"), "ready");
}

#[test]
fn expect_on_empty_cell_panics_with_guidance() {
    let cell: Late<u32> = Late::new();
    let err = std::panic::catch_unwind(|| {
        cell.expect("grpc backend");
    })
    .unwrap_err();
    let msg = err
        .downcast_ref::<String>()
        .expect("panic payload should be a String");
    assert!(msg.contains("grpc backend"), "names the value: {msg}");
    assert!(msg.contains("u32"), "names the type: {msg}");
    assert!(
        msg.contains("before it was filled"),
        "points at the lifecycle: {msg}"
    );
}

#[test]
fn default_is_empty() {
    let cell: Late<u8> = Late::default();
    assert!(cell.get().is_none());
}

#[test]
fn debug_shows_fill_state() {
    let cell: Late<u32> = Late::new();
    assert_eq!(format!("{cell:?}"), "Late(<unfilled>)");
    cell.fill(3).unwrap();
    assert_eq!(format!("{cell:?}"), "Late(3)");
}

#[r2e_core::test]
async fn the_keep_alive_layer_preserves_the_body_size_hint() {
    // The wrapper must be length-transparent. A combinator that reports an
    // unknown length (`BodyExt::map_frame` does — it cannot know the mapping
    // preserved sizes) costs every response its `Content-Length` and puts hyper
    // into chunked transfer encoding, framework-wide.
    use http_body::Body as _;

    let app = AppBuilder::new()
        .plugin(GraphProbePlugin)
        .provide(Alpha(21))
        .build_state()
        .await;

    let response = crate::support::raw(
        app.build(),
        "GET",
        "/after-yield",
        &[],
        r2e_core::http::Body::empty(),
    )
    .await;

    assert_eq!(
        response.body().size_hint().exact(),
        Some("alive-21".len() as u64),
        "the graph-carrying body must forward the inner size hint"
    );
}
