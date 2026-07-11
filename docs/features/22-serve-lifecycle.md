# Feature 22 — Serve Lifecycle: Programmatic Stop & Awaited Graceful Drain

## Goal

Give the server a real shutdown contract, without hand-rolled drain plumbing:

- **`StopHandle`** — stop a running server programmatically (tests, embedded servers, admin endpoints), triggering the exact same graceful shutdown as Ctrl-C/SIGTERM. No more `abort()`-ing the serve task.
- **`on_drain`** — awaited hooks that run when shutdown is *triggered*, **before** the listener stops accepting: flip a readiness endpoint, wait for load-balancer deregistration, broadcast a drain notice.
- **`ServeContext`** — serve hooks (plugin API) receive the real app shutdown token and can `track()` spawned server tasks so their drain is *awaited* at shutdown (bounded by `shutdown_grace_period`) instead of being cancelled and forgotten. The gRPC separate-port server uses this: its drain completes before the process exits.

## The shutdown sequence

On OS signal **or** `StopHandle::stop()`:

1. **`on_drain` hooks** are awaited, in registration order. The server is still accepting and serving normally.
2. **Plugin shutdown hooks** fire (sync — cancel tokens for scheduler/`spawn_service` tasks), then **plugin async shutdown hooks** are awaited (e.g. executor graceful drain).
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
    let handle = r2e_core::rt::spawn(async move {
        my_server(shutdown).await;                                 // drain on cancellation
    });
    serve_ctx.track(handle);                                       // drain awaited at step 4
});
```

- `task_registry()` — the shared `TaskRegistryHandle` (scheduled tasks, tagged subsystem tasks).
- `shutdown_token()` — the app shutdown `CancellationToken`; cancelled when the graceful drain begins.
- `track(handle)` — the handle joins the post-drain await set (same pool as `spawn_service` handles), bounded by `shutdown_grace_period`.

**Track any server-like task that drains on the shutdown token.** An untracked task may be killed mid-drain when the process exits.

### gRPC drain

`GrpcServer::on_port(...)` (separate-port transport) now rides this contract: the tonic server observes the app shutdown token via `serve_with_incoming_shutdown` and its handle is tracked — at shutdown you'll see `Awaiting background tasks to finish count=1` and the gRPC in-flight calls complete before `run()` returns. The multiplexed transport rides the HTTP drain as before.

## Interactions

- **Sharded serving (`server.workers`)**: the stop handle works identically — workers observe the shared token's cancellation. A cancel-on-drop guard inside the shutdown future guarantees the token fires even if a drain/plugin hook panics.
- **QUIC**: the HTTP/3 endpoint drains on the same token; its task handle joins the tracked set, so the QUIC drain is awaited in step 4 and bounded by `shutdown_grace_period`.
- **`shutdown_grace_period`**: bounds step 4 (tracked handles + `on_stop` hooks). Without one, shutdown waits indefinitely for tracked drains — a client holding a server-streaming gRPC call open holds the (grace-boundable) tracked drain, and an open HTTP SSE/streaming response holds the HTTP drain itself (step 3, never grace-bounded — same as plain axum). `on_drain` hooks are **not** bounded by it — they run before the drain begins.
- **dev-reload**: plugin shutdown hooks are skipped on hot-reload re-entry (unchanged); `on_drain`/`on_stop` user hooks always run (same rule as `on_stop` had before).

## Files

- `r2e-core/src/lifecycle.rs` — `StopHandle`, `DrainHook`
- `r2e-core/src/builder/mod.rs` — `ServeContext`, `with_stop_handle`
- `r2e-core/src/builder/typed.rs` — `on_drain`
- `r2e-core/src/builder/prepared.rs` — `stop_handle()`, shutdown sequencing in `run_inner`
- `r2e-grpc/src/server.rs` — tracked gRPC drain
- `r2e-core/tests/builder_prepared.rs`, `examples/example-grpc/tests/grpc_serve.rs` — proof
