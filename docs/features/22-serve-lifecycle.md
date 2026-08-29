# Feature 22 — Serve Lifecycle: Programmatic Stop & Awaited Graceful Drain

## TL;DR

A real shutdown contract without hand-rolled drain plumbing. `StopHandle::stop()` triggers the same graceful shutdown as Ctrl-C / SIGTERM — provide it as a bean for an admin/stop endpoint, or get it from `prepared.stop_handle()`. `on_drain` hooks are awaited *before* the listener stops accepting (flip readiness, wait for load-balancer deregistration); `ServeContext::track()` lets serve hooks have spawned tasks (gRPC, QUIC) drained rather than cancelled, bounded by `shutdown_grace_period`. Sequence: on_drain → plugin shutdown → stop accepting (in-flight finish, bounded by `drain_timeout` — 30s by default, `server.drain-timeout` to tune) → await tracked tasks (bounded by `shutdown_grace_period`, per handle) → on_stop (always runs). Upgraded WebSocket sessions are tracked tasks too: `WsStream::next()` sends a `1001 Going Away` frame and ends the loop at shutdown.


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
4. After the HTTP drain: tracked task handles (`spawn_service`, `ServeContext::track`, upgraded WebSocket sessions) are awaited **concurrently**, each bounded on its own by `shutdown_grace_period` if set.
5. **`on_stop` hooks** run, in registration order. They are **outside every budget** — see below.
6. `run()` / `serve()` / `serve_auto()` resolves `Ok(())`.

### Which bound covers what

Two independent budgets, each covering exactly one phase:

| Phase | Bounded by | Default | On overflow |
|---|---|---|---|
| 1. `on_drain` hooks | — | — | — |
| 2. plugin sync + async shutdown hooks (incl. `#[pre_destroy]`) | — | — | — |
| 3. HTTP drain — in-flight requests after the listener stops accepting | **`drain_timeout`** (builder) or **`server.drain-timeout`** (config) | **30s** — `drain_timeout_unbounded()` is the explicit opt-out | `warn!(phase = "http drain", …)`, remaining connections abandoned, shutdown continues |
| 4. tracked-handle join (`spawn_service`, `ServeContext::track`, gRPC/QUIC drains, WebSocket sessions) | **`shutdown_grace_period`**, applied **per handle** | `None` (wait indefinitely) | `warn!(phase = "tracked-handle join", service = <name>, …)`, that handle abandoned (detached, not aborted), the others keep their own budget |
| 5. `on_stop` hooks | — **always run** | — | — |

Consequences worth spelling out:

- **`on_stop` is must-run.** It runs even when the drain timed out *and* every tracked handle overflowed its grace period. Put application-state reconciliation there (mark interrupted runs cancelled, release an advisory lock) and it will happen.
- **`shutdown_grace_period` is per handle, not for the whole phase.** One service that ignores its token costs one grace period, not the budget of everything registered after it, and phase 4 as a whole still costs at most one grace period because the handles are joined concurrently.
- **The warning names the culprit.** Handles carry a label: `spawn_service::<C>()` uses `C`'s type name, `ServeContext::track_named(name, fut)` uses the name you give, a generated `#[ws(...)]` session uses `ws:<Controller>::<method>`, plain `track(fut)` shows `<unnamed>`.
- **`drain_timeout` is measured from cancellation**, not from `serve()` — the clock starts when the listener stops accepting.
- **The drain is bounded by default (30s).** An app that never mentions `drain_timeout` still terminates: the default matches Spring's `spring.lifecycle.timeout-per-shutdown-phase`. Precedence is `.drain_timeout(d)` / `.drain_timeout_unbounded()` > `server.drain-timeout` > 30s, and an unparseable config value fails the boot (like any other invalid `server.*` value) rather than silently becoming something else.

```yaml
server:
  drain-timeout: 10s      # integer = seconds; "500ms" / "2m" / "1h" also parse
```

```rust
AppBuilder::new()
    .drain_timeout(Duration::from_secs(10))   // wins over server.drain-timeout
    // .drain_timeout_unbounded()             // …or opt out of the bound entirely
```

- **Unbounded is code-only.** No config value means "unbounded" — `server.drain-timeout` always sets a bound. Waiting forever (one open SSE stream can keep the process alive indefinitely, and no other budget rescues it) is a deliberate decision, spelled `drain_timeout_unbounded()`.
- **Both strategies behave identically.** Under SO_REUSEPORT sharded serving (`server.workers`) each worker bounds its own drain on its own child token; since all child tokens are cancelled by the same parent, the whole set still finishes within `drain_timeout` of the signal.

## `StopHandle`

```rust
use r2e::prelude::*;

let app = AppBuilder::new().build_state().await;
let prepared = app.prepare("127.0.0.1:8080");
let stop = prepared.stop_handle();          // Clone-able

let server = r2e::rt::spawn(prepared.run());

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

`prepare() → stop_handle() → run()` replaces `r2e::rt::spawn(app.serve(..)) + handle.abort()`. The test exercises the *real* shutdown path and asserts a clean exit:

```rust
let prepared = app.prepare(&format!("127.0.0.1:{port}"));
let stop = prepared.stop_handle();
let server = r2e::rt::spawn(async move { prepared.run().await.map_err(|e| e.to_string()) });
// ... requests ...
stop.stop();
assert!(r2e::rt::timeout(Duration::from_secs(5), server).await.unwrap().unwrap().is_ok());
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
        r2e::rt::sleep(Duration::from_secs(5)).await;      // LB notices, deregisters
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
- `track(fut)` / `track_named(name, fut)` — spawns `fut` and joins the post-drain await set (same pool as `spawn_service` handles), bounded per handle by `shutdown_grace_period`. Prefer `track_named`: the label is what the grace-period warning prints (`service = …`); plain `track` shows `<unnamed>`.

It takes the **future**, not a `JobHandle` (breaking — pass the `async` block instead of `rt::spawn(...)`): the task is wrapped so it owns a strong reference to the bean graph for its whole lifetime. That matters because the await set is best-effort — an elapsed `shutdown_grace_period` abandons that handle (it is detached, never aborted), and a dropped `run()` future (an `r2e dev` hot patch) joins nothing at all — so a task that resolves beans through a `GraphHandle` must carry the graph itself, which a pre-spawned handle cannot be given after the fact.

**Every task the serve hooks start must go through `track`** — that is how in-tree plugins run the gRPC listener, the scheduler driver, the live-config watch supervisor and the tenant sweeper. A bare `rt::spawn` is outside the model twice over: nothing cancels it, nothing waits for it, and it does not keep the graph alive. (`r2e_scheduler::start_jobs` is the documented exception: a standalone driver for tests, whose lifetime you own through its cancellation token.)

**A boot that aborts winds tracked work down too.** If a startup hook returns `Err` (they run *after* the serve hooks) or serving itself fails, `run()` cancels the shutdown token, fires the plugin cancel hooks, and awaits the tracked handles — each bounded by `shutdown_grace_period` when set — before returning the error. So a tracked task that waits on the token is never left holding a port. User `on_drain`/`on_stop` hooks and `#[pre_destroy]` disposers do **not** run on that path: the app never served.

### gRPC drain

`GrpcServer::on_port(...)` (separate-port transport) now rides this contract: the tonic server observes the app shutdown token via `serve_with_incoming_shutdown` and its handle is tracked — at shutdown you'll see `Awaiting background tasks to finish count=1` and the gRPC in-flight calls complete before `run()` returns. The multiplexed transport rides the HTTP drain as before.

### WebSocket sessions

An upgraded WebSocket is invisible to both budgets unless the framework puts it back in view: for hyper the connection is *finished* the moment the upgrade is handed over (so the HTTP drain does not wait for it), and the future axum spawns for `on_upgrade` is detached (so nothing joins it). Before this, a session was simply killed by runtime teardown after `run()` returned — no close frame, no chance to reconcile per-session state.

Every session opened by a generated `#[ws(...)]` route now runs on the **tracked lane**, so it behaves like any other tracked task:

- the session **owns the bean graph** while it runs — resolving beans mid-loop, after `run()` has begun shutting down, is fine;
- its handle is joined in step 4, bounded on its own by `shutdown_grace_period`, and named `ws:<Controller>::<method>` in the overflow warning;
- `on_stop` (step 5) therefore runs **after** sessions that end in time.

`WsStream` observes the app shutdown token, so the ordinary loop shape needs no changes to be well-behaved:

```rust
#[ws("/chat")]
async fn chat(&self, mut ws: WsStream) {
    while let Some(Ok(msg)) = ws.next().await {
        // ... echo, broadcast, whatever
    }
    // reached at shutdown: `next()` sends a `1001 Going Away` close frame
    // and then reports end-of-stream.
}
```

When shutdown is triggered, `WsStream::next()` sends the peer a close frame with code **1001 Going Away** and returns `None` — the loop exits, the handler returns, the handle is joined. `ws.shutdown_requested().await` is the same signal as a bare future (for a `select!` on the send side — it borrows nothing, so holding it across an await keeps the session `Send`), and `ws.shutdown_token()` exposes the token itself.

A handler that never touches the receive side (or ignores the end-of-stream) gets nothing worse than any other stubborn task: after `shutdown_grace_period` it is abandoned — detached, not aborted — with `warn!(phase = "tracked-handle join", service = "ws:MyController::my_method", …)`, and shutdown proceeds to `on_stop`. With no grace period configured, shutdown waits for it indefinitely.

An app that is never served through `run()` — `build_with_consumers()`, `TestApp`, or a router handed to `axum::serve` by hand — has no tracked lane to arm, and sessions there run detached exactly as they always did. Nothing panics; there is simply nothing to join them against.

Under sharded serving (`server.workers`) the guarantee is the same, and it is not free: the socket was accepted by a *worker* runtime while the session runs on the control plane, so the worker's I/O driver has to outlive the session. Workers therefore **park** once their HTTP drain is over and their per-worker services are down — they stay inside `block_on`, driving that I/O driver — until the control plane has finished step 4, and only then drop their runtime. Without that handshake a session that reacted slowly (anything past the first poll after cancellation) would find its socket dead well inside its own grace period, and the peer would see a TCP reset instead of the close frame.

## Interactions

- **Sharded serving (`server.workers`)**: the stop handle works identically — workers observe the shared token's cancellation. A cancel-on-drop guard inside the shutdown future guarantees the token fires even if a drain/plugin hook panics. `drain_timeout` is applied by each worker to its own drain, on its own child token, so the sharded and single-listener strategies bound the drain the same way. Worker runtimes are dropped **after** step 4, not at the end of their serve loop, so tracked work that owns a socket a worker accepted (a WebSocket session) still has a live I/O driver for its whole grace period.
- **QUIC**: the HTTP/3 endpoint drains on the same token; its task handle joins the tracked set (label `quic endpoint drain`), so the QUIC drain is awaited in step 4 and bounded by `shutdown_grace_period`.
- **`shutdown_grace_period`**: bounds step 4 only (the tracked-handle join), per handle. Without one, shutdown waits indefinitely for tracked drains — a client holding a server-streaming gRPC call open holds the tracked drain. It does **not** bound `on_drain` (step 1, before the drain begins), the HTTP drain (step 3 — that is `drain_timeout`), or `on_stop` (step 5, which always runs).
- **`drain_timeout`**: bounds step 3 only, and does so by default (30s, or `server.drain-timeout`). When the budget elapses the remaining connections are abandoned and shutdown proceeds to steps 4 and 5. Only `drain_timeout_unbounded()` removes the bound — then an open HTTP SSE/streaming response or a slow handler holds the drain forever, exactly like plain axum.
- **WebSockets**: sessions from `#[ws(...)]` routes are tracked (see above), so they hold step 4 rather than escaping shutdown — bounded per session by `shutdown_grace_period`. They do **not** hold the HTTP drain (step 3): for hyper the connection ended at the upgrade. Under `server.workers` they also hold their worker's runtime open for the length of step 4 (see above).
- **Tests**: `TestApp::boot` runs the startup phase and keeps the app alive as a `RunningApp`, so `app.shutdown().await` executes this whole sequence — steps 1 to 5, under the app's own `drain_timeout` and `shutdown_grace_period`. A server from `app.serve()` runs on the tracked lane and drains with it. Only the plugin **serve hooks** are skipped in process (they bind ports and start the scheduler driver), so `ServeContext::track` work is the one part of step 4 a test does not exercise. See `docs/features/12-testing.md` and `r2e-test/tests/lifecycle.rs`.
- **dev-reload**: a hot patch **drops** the previous `run()` future instead of stopping it, so no shutdown sequence runs for that cycle at all — no `on_drain`, no plugin `on_shutdown*`, no `on_stop`, no `#[pre_destroy]`. What does happen is cancellation: the dropped future's guard cancels that cycle's shutdown token, and every token derived from it (each `spawn_service` task, the relayed scheduler driver) is cancelled with it, so the cycle's background work stops even though nothing joins it. Hooks registered on re-entry are likewise skipped for cycles ≥ 2 (serve/startup lifecycle runs once). The full sequence above applies to a real stop: signal, `StopHandle::stop()`, or `r2e dev` exiting.

## Files

- `r2e-core/src/lifecycle.rs` — `StopHandle`, `DrainHook`
- `r2e-core/src/builder/mod.rs` — `ServeContext`, `with_stop_handle`
- `r2e-core/src/builder/typed.rs` — `on_drain`, `drain_timeout`, `drain_timeout_unbounded`
- `r2e-core/src/runtime/drain.rs` — `bounded_http_drain`, `DEFAULT_DRAIN_TIMEOUT`, `resolve_drain_timeout`
- `r2e-core/src/builder/prepared.rs` — `stop_handle()`, shutdown sequencing in `run_inner`, the startup phase shared with the in-process path (`start_lifecycle`, `start_in_process`)
- `r2e-core/src/builder/running.rs` — `RunningApp`: the in-process app (`TestApp`), same startup and shutdown, no serve hooks
- `r2e-core/src/builder/ws_sessions.rs` — `WsSessions`, the tracked lane for upgraded sockets (§ "Sharded serving" holds the worker-parking invariant)
- `r2e-core/src/runtime/sharded.rs` — `WorkerPark`: workers outlive the tracked-handle join
- `r2e-core/src/web/ws.rs` — `WsStream` shutdown observation + the `1001 Going Away` frame
- `r2e-grpc/src/server.rs` — tracked gRPC drain
- `r2e-core/tests/runtime/shutdown_budget.rs`, `r2e-core/tests/runtime/ws_shutdown.rs`, `examples/example-grpc/tests/grpc_serve.rs` — proof
