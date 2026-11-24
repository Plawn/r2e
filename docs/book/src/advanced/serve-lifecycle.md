# Serve Lifecycle and Graceful Shutdown

R2E has a fixed shutdown contract. Ctrl-C, SIGTERM and a programmatic
`StopHandle::stop()` all run the same graceful sequence: pre-drain hooks, an
HTTP drain with a time limit, a join of background tasks, and `on_stop` hooks
that always run.

## Setup

No Cargo feature is needed. Everything on this page is part of `r2e-core`.

## The shutdown sequence

1. **`on_drain` hooks** are awaited in registration order. The server still
   accepts and serves requests.
2. **Plugin shutdown hooks** and `#[pre_destroy]` run. Background services on
   the default "early" lane (`spawn_service`, the scheduler) are cancelled.
3. **HTTP drain.** The listener stops accepting and in-flight requests finish,
   bounded by `drain_timeout` (30 s by default).
4. **Tracked tasks are joined**: `spawn_service` handles, `ServeContext::track`
   tasks (gRPC, QUIC), WebSocket sessions. Each is bounded by
   `shutdown_grace_period` if one is set.
5. **After-drain services** (`#[service(stop = "after_drain")]`) stop one by one
   in `stop_order`, then the executor pool drains.
6. **`on_stop` hooks** run. They always run, even if earlier steps timed out.

| Phase | Bound | Default |
|---|---|---|
| HTTP drain (3) | `drain_timeout` / `server.drain-timeout` | 30 s |
| Tracked-task join (4), per task | `shutdown_grace_period` | none (waits) |
| After-drain services (5), per service | `shutdown_grace_period` | none (waits) |
| `on_drain` (1), `on_stop` (6) | none | — |

When a bound is exceeded, R2E logs a warning naming the phase and the task, then
continues.

## Drain timeout

```yaml
server:
  drain-timeout: 10s   # integer = seconds; "500ms", "2m" also parse
```

```rust
AppBuilder::new()
    .drain_timeout(Duration::from_secs(10))   // wins over server.drain-timeout
    // .drain_timeout_unbounded()             // or wait forever, on purpose
    .shutdown_grace_period(Duration::from_secs(5))
```

An unbounded drain is code-only: one open streaming response could otherwise
keep the process alive forever.

## `on_drain` and `on_stop`

`on_drain` runs before the listener stops accepting. Use it to mark the app as
not ready and give the load balancer time to deregister it:

```rust
AppBuilder::new()
    .provide(readiness.clone())
    .build_state()
    .await
    .on_drain(|state| async move {
        state.bean::<Readiness>().unwrap().set_draining(); // health endpoint -> unready
        r2e::rt::sleep(Duration::from_secs(5)).await;      // LB notices, deregisters
    })
    .on_stop(|_state| async move {
        tracing::info!("drained and stopped");
    })
    .serve_auto()
    .await?;
```

Put must-happen cleanup (mark interrupted jobs, release a lock) in `on_stop`.

## `StopHandle`: stop programmatically

```rust
let app = AppBuilder::new().build_state().await;
let prepared = app.prepare("127.0.0.1:8080");
let stop = prepared.stop_handle();            // Clone

let server = r2e::rt::spawn(prepared.run());

// later, from anywhere:
stop.stop();                                   // triggers graceful shutdown
server.await.unwrap().unwrap();                // resolves after the full drain
```

`StopHandle` offers `new()`, `stop()` (idempotent, non-blocking),
`is_stopped()` and `stopped().await`.

To stop from an admin endpoint, provide it as a bean and inject it:

```rust
let stop = StopHandle::new();

AppBuilder::new()
    .provide(stop)               // #[inject] stop: StopHandle
    .build_state()
    .await
    .register_controller::<AdminController>()
    .serve_auto()
    .await?;
```

`prepare()` picks the handle in this order: `with_stop_handle(..)`, then a
`StopHandle` bean, then a fresh one.

## Reacting to shutdown in your code

`r2e::rt::ShutdownToken` is a bean on every builder. Inject it into a bean that
runs its own loop:

```rust
use r2e::rt::ShutdownToken;

#[bean]
impl Poller {
    #[new]
    fn new(#[inject] shutdown: ShutdownToken) -> Self { Self { shutdown } }

    async fn run(&self) {
        loop {
            r2e::rt::select! {
                _ = self.shutdown.cancelled() => break,
                _ = r2e::rt::sleep(Duration::from_secs(1)) => { /* work */ }
            }
        }
    }
}
```

Built-in behaviour:

- `#[sse]` streams end when the drain starts, so an idle subscriber does not
  hold the drain for 30 s.
- `#[ws]` sessions: `WsStream::next()` sends a `1001 Going Away` close frame
  and returns `None`.

## Plugin serve hooks: `ServeContext`

Plugins that run their own server get a `ServeContext` in `on_serve`:

| Method | Purpose |
|---|---|
| `shutdown_token()` | Cancelled when the drain begins |
| `track(fut)` / `track_named(name, fut)` | Spawn a task joined in step 4 (prefer `track_named` for clear warnings) |
| `bind_tcp(owner, addr)` | Bind a listener that survives `r2e dev` hot patches |
| `task_registry()` | Shared task registry |

## Testing

`TestApp::boot` keeps the app running. `app.shutdown().await` runs the whole
sequence with the app's own `drain_timeout` and `shutdown_grace_period`, so
`on_drain`, `#[pre_destroy]` and `on_stop` are all exercised:

```rust
#[r2e::test]
async fn stops_cleanly() {
    let app = TestApp::boot::<MyApp>().await;
    app.get("/health").send().await.assert_ok();
    app.shutdown().await;
}
```

Plugin serve hooks, sharded serving and QUIC do not run in-process. To test
those, start a real server with `prepare(..)` and stop it with its
`StopHandle`.

Under `r2e dev`, a hot patch drops the old cycle instead of stopping it: no
hooks run, but every shutdown token is cancelled.

## See also

- Full guide: [docs/features/22-serve-lifecycle.md](https://github.com/plawn/r2e/blob/master/docs/features/22-serve-lifecycle.md)
- [Lifecycle Hooks](./lifecycle-hooks.md)
- [Background Services](../events-and-scheduling/background-services.md)
