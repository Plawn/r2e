# Feature 22 — Serve Lifecycle: Programmatic Stop & Awaited Graceful Drain

## TL;DR

A real shutdown contract without hand-rolled drain plumbing. `StopHandle::stop()` triggers the same graceful shutdown as Ctrl-C / SIGTERM — provide it as a bean for an admin/stop endpoint, or get it from `prepared.stop_handle()`. `on_drain` hooks are awaited *before* the listener stops accepting (flip readiness, wait for load-balancer deregistration); `ServeContext::track()` lets serve hooks have spawned tasks (gRPC, QUIC) drained rather than cancelled, bounded by `shutdown_grace_period`. Sequence: on_drain → plugin shutdown → stop accepting (in-flight finish) → await tracked tasks → on_stop.


## Goal

Give the server a real shutdown contract, without hand-rolled drain plumbing:

- **`StopHandle`** — stop a running server programmatically (tests, embedded servers, admin endpoints), triggering the exact same graceful shutdown as Ctrl-C/SIGTERM. No more `abort()`-ing the serve task.
- **`on_drain`** — awaited hooks that run when shutdown is *triggered*, **before** the listener stops accepting: flip a readiness endpoint, wait for load-balancer deregistration, broadcast a drain notice.
- **`ServeContext`** — serve hooks (plugin API) receive the real app shutdown token and can `track()` spawned server tasks so their drain is *awaited* at shutdown (bounded by `shutdown_grace_period`) instead of being cancelled and forgotten. The gRPC separate-port server uses this: its drain completes before the process exits.

## The shutdown sequence

On OS signal **or** `StopHandle::stop()`:

1. **`on_drain` hooks** are awaited, in registration order. The server is still accepting and serving normally.
2. **Plugin shutdown hooks** fire (sync — cancel tokens for scheduler/`spawn_service` tasks), then **plugin async shutdown hooks** are awaited (e.g. executor graceful drain). Sync hooks run in registration order, one at a time and each panic-isolated, so one bad hook cannot silence the rest. They control *when* background work stops (early, before the HTTP drain), not *whether*: every framework shutdown token is either a child of the app shutdown token (`spawn_service`'s per-service token) or explicitly relayed from it (the scheduler driver, whose token is a plugin bean), so an abnormal exit that runs no hook at all (a panic, or an `r2e dev` hot patch dropping the `run()` future) still cancels them through that token's drop guard.
3. The shared shutdown token is cancelled: the HTTP listener stops accepting; in-flight requests finish. Tracked server tasks (gRPC, QUIC) observe the same token and drain **concurrently**.
4. After the HTTP drain: tracked task handles (`spawn_service`, `ServeContext::track`) are awaited, then **`on_stop` hooks** run. Both bounded together by `shutdown_grace_period` if set.
5. `run()` / `serve()` / `serve_auto()` resolves `Ok(())`.

## `StopHandle`

```rust
use r2e::prelude::*;

let app = AppBuilder::new().build_state().await;
let prepared = app.prepare("127.0.0.1:8080");
let stop = prepared.stop_handle();          // Clone-able

let server = tokio::spawn(prepared.run());

// ... later, from anywhere:
stop.stop();                                 // triggers graceful shutdown
server.await.unwrap().unwrap();              // resolves after the full drain
```

API: `StopHandle::new()`, `stop()` (idempotent, non-blocking), `is_stopped()`, `stopped().await`.

### As a bean (admin/stop endpoint)

Providing a `StopHandle` bean is enough — `prepare()` picks it up automatically:

```rust
let stop = StopHandle::new();

AppBuilder::new()
    .provide(stop)                  // injectable: #[inject] stop: StopHandle
    .build_state()
    .await
    .register_controller::<AdminController>()
    .serve_auto()
    .await?;
```

Resolution order at `prepare()`: explicit `with_stop_handle()` → `StopHandle` bean from the graph → fresh handle (returned by `PreparedApp::stop_handle()`).

### In e2e tests

`prepare() → stop_handle() → run()` replaces `tokio::spawn(app.serve(..)) + handle.abort()`. The test exercises the *real* shutdown path and asserts a clean exit:

```rust
let prepared = app.prepare(&format!("127.0.0.1:{port}"));
let stop = prepared.stop_handle();
let server = tokio::spawn(async move { prepared.run().await.map_err(|e| e.to_string()) });
// ... requests ...
stop.stop();
assert!(tokio::time::timeout(Duration::from_secs(5), server).await.unwrap().unwrap().is_ok());
```

## `on_drain` — awaited pre-drain hooks

`on_stop` runs *after* the drain; `on_drain` runs *at shutdown trigger, before the server stops accepting*. This is where "prepare the outside world for our departure" work belongs — the readiness-flip + deregistration-wait pattern that previously required hand-rolled `begin_drain`/`wait_in_flight` plumbing:

```rust
AppBuilder::new()
    .provide(readiness.clone())
    .build_state()
    .await
    .on_drain(|state| async move {
        state.bean::<Readiness>().unwrap().set_draining();     // health endpoint → unready
        tokio::time::sleep(Duration::from_secs(5)).await;      // LB notices, deregisters
    })
    .on_stop(|_state| async move {
        tracing::info!("drained and stopped");
    })
    .serve_auto()
    .await?;
```

Signature mirrors `on_stop`: `FnOnce(T) -> Future<Output = ()>`, awaited in registration order. While drain hooks run, in-flight **and new** requests are still served.

## `ServeContext` — plugin serve hooks (breaking change)

`DeferredContext::on_serve` hooks now receive a `ServeContext` instead of `(TaskRegistryHandle, CancellationToken)` — the old token was a fresh one nobody ever cancelled:

```rust
dctx.on_serve(move |serve_ctx| {
    let tasks = serve_ctx.task_registry().take_of::<MyMarker>();   // shared task registry
    let shutdown = serve_ctx.shutdown_token();                     // cancelled at step 3 above
    serve_ctx.track(async move {                                   // spawned + drain awaited at step 4
        my_server(shutdown).await;                                 // drain on cancellation
    });
});
```

- `task_registry()` — the shared `TaskRegistryHandle` (scheduled tasks, tagged subsystem tasks).
- `shutdown_token()` — the app shutdown token; cancelled when the graceful drain begins. Since the `r2e-rt` extraction it is an `r2e::rt::CancelToken` (**breaking**), not a `tokio_util::sync::CancellationToken` — apps no longer need `tokio-util` in their own manifest. It behaves the same (`cancelled().await` is cancellation-safe, `child_token()`, `drop_guard()`); a call site that genuinely needs the raw tokio-util token converts with `.into()` / `.into_inner()`.
- `track(fut)` — spawns `fut` and joins the post-drain await set (same pool as `spawn_service` handles), bounded by `shutdown_grace_period`.

It takes the **future**, not a `JobHandle` (breaking — pass the `async` block instead of `rt::spawn(...)`): the task is wrapped so it owns a strong reference to the bean graph for its whole lifetime. That matters because the await set is best-effort — an elapsed `shutdown_grace_period` drops the join futures, and a dropped `run()` future (an `r2e dev` hot patch) joins nothing at all — so a task that resolves beans through a `GraphHandle` must carry the graph itself, which a pre-spawned handle cannot be given after the fact.

**Every task the serve hooks start must go through `track`** — that is how in-tree plugins run the gRPC listener, the scheduler driver, the live-config watch supervisor and the tenant sweeper. A bare `rt::spawn` is outside the model twice over: nothing cancels it, nothing waits for it, and it does not keep the graph alive. (`r2e_scheduler::start_jobs` is the documented exception: a standalone driver for tests, whose lifetime you own through its cancellation token.)

**A boot that aborts winds tracked work down too.** If a startup hook returns `Err` (they run *after* the serve hooks) or serving itself fails, `run()` cancels the shutdown token, fires the plugin cancel hooks, and awaits the tracked handles — bounded by `shutdown_grace_period` when set — before returning the error. So a tracked task that waits on the token is never left holding a port. User `on_drain`/`on_stop` hooks and `#[pre_destroy]` disposers do **not** run on that path: the app never served.

### gRPC drain

`GrpcServer::on_port(...)` (separate-port transport) now rides this contract: the tonic server observes the app shutdown token via `serve_with_incoming_shutdown` and its handle is tracked — at shutdown you'll see `Awaiting background tasks to finish count=1` and the gRPC in-flight calls complete before `run()` returns. The multiplexed transport rides the HTTP drain as before.

## Interactions

- **Sharded serving (`server.workers`)**: the stop handle works identically — workers observe the shared token's cancellation. A cancel-on-drop guard inside the shutdown future guarantees the token fires even if a drain/plugin hook panics.
- **QUIC**: the HTTP/3 endpoint drains on the same token; its task handle joins the tracked set, so the QUIC drain is awaited in step 4 and bounded by `shutdown_grace_period`.
- **`shutdown_grace_period`**: bounds step 4 (tracked handles + `on_stop` hooks). Without one, shutdown waits indefinitely for tracked drains — a client holding a server-streaming gRPC call open holds the (grace-boundable) tracked drain, and an open HTTP SSE/streaming response holds the HTTP drain itself (step 3, never grace-bounded — same as plain axum). `on_drain` hooks are **not** bounded by it — they run before the drain begins.
- **dev-reload**: a hot patch **drops** the previous `run()` future instead of stopping it, so no shutdown sequence runs for that cycle at all — no `on_drain`, no plugin `on_shutdown*`, no `on_stop`, no `#[pre_destroy]`. What does happen is cancellation: the dropped future's guard cancels that cycle's shutdown token, and every token derived from it (each `spawn_service` task, the relayed scheduler driver) is cancelled with it, so the cycle's background work stops even though nothing joins it. Hooks registered on re-entry are likewise skipped for cycles ≥ 2 (serve/startup lifecycle runs once). The full sequence above applies to a real stop: signal, `StopHandle::stop()`, or `r2e dev` exiting.

## Files

- `r2e-core/src/lifecycle.rs` — `StopHandle`, `DrainHook`
- `r2e-core/src/builder/mod.rs` — `ServeContext`, `with_stop_handle`
- `r2e-core/src/builder/typed.rs` — `on_drain`
- `r2e-core/src/builder/prepared.rs` — `stop_handle()`, shutdown sequencing in `run_inner`
- `r2e-grpc/src/server.rs` — tracked gRPC drain
- `r2e-core/tests/builder_prepared.rs`, `examples/example-grpc/tests/grpc_serve.rs` — proof
