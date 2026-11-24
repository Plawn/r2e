# Sharded Serving

By default R2E serves HTTP from one listener on a multi-threaded,
work-stealing runtime. With `server.workers`, it starts **N worker threads**
instead. Each one runs its own single-threaded runtime and its own
`SO_REUSEPORT` listener on the same address, and the kernel spreads incoming
connections across them. Your controllers, guards and Axum-level layers do not
change: every worker serves a clone of the same router.

## Setup

There is no Cargo feature. Sharded serving is available on Unix targets (except
Solaris, illumos and Cygwin) and is turned on by configuration:

```yaml
server:
  host: "0.0.0.0"
  port: 3000
  workers: 4        # or "per-core"
```

| Value | Effect |
|---|---|
| *(absent)* | Single listener (default) |
| integer `n >= 1` | `n` workers (even `1`) |
| `"per-core"` | One worker per available core |
| `0`, negative, `> 1024`, other strings | Hard error at `run()`, never a silent fallback |

On an unsupported platform, setting `server.workers` is an error.

## When to use it

Start on the default runtime. Sharding gives up work-stealing's automatic load
balancing in exchange for per-core locality, which only pays off when:

- serving is CPU-bound at high request rates (serialization, parsing),
- the machine has many cores,
- requests are short and similar in cost,
- the hot path does not contend on a single shared lock.

It does not help, and can be slower, for low or bursty traffic, IO-bound
handlers (database, upstream HTTP), or long and uneven requests. A connection
stays on the worker that accepted it, so one slow request delays every other
connection on that core. **Benchmark your own workload before and after.**

## Limitations

- Ignored, with a warning, under `r2e dev` (dev-reload) and with
  `run_with_listener`.
- R2E's QUIC/HTTP3 endpoint is not sharded; only TCP is.
- If one worker dies, the others keep serving at reduced capacity. Nothing
  restarts it.
- Scheduler, executor, event consumers and the rest of the app run on the
  "control plane" runtime, not on the workers.

## Worker identity: `WorkerInfo`

`WorkerInfo` tells code which worker it runs on. Use it as a handler parameter,
or call `WorkerInfo::current()` anywhere:

```rust
#[get("/whoami")]
async fn whoami(&self, info: WorkerInfo) -> String {
    format!("{info}") // "worker 3/8" or "control-plane"
}
```

It exposes `id()`, `workers()`, `role()` and `is_data_plane()`. In a
non-sharded app it reports `control-plane`.

## One value per worker: `WorkerLocal<T>`

`WorkerLocal<T>` is a `Clone + Send + Sync` handle that fronts one `T` per
worker thread. `T` may be `!Send` (`Rc`, `RefCell`, a socket):

```rust
AppBuilder::new()
    .load_config::<()>()
    .worker_local(|worker: WorkerContext| async move {
        Ok(RefCell::new(ShardStats::new(worker.id())))
    })
```

Inject the handle with `#[inject]` and read the current worker's value with
`with(|t| ..)` (panics off a worker) or `try_with(|t| ..)` (returns `None`).

## Per-worker services

For work that must live inside a worker, such as a shard-local UDP or QUIC
socket, register a factory with `per_worker_service`. It runs once on each
worker thread, before that worker accepts connections:

```rust
use r2e::prelude::*;
use r2e::runtime::worker::BoxError;

AppBuilder::new()
    .load_config::<()>()
    .build_state()
    .await
    .register_controller::<PingController>()
    .per_worker_service(move |worker: WorkerContext| async move {
        let counter = Rc::new(RefCell::new(0u64)); // !Send is fine here
        let id = worker.id();
        let shutdown = worker.shutdown();
        worker.spawn_local(async move {
            shutdown.cancelled().await;
            tracing::info!(worker = id, count = *counter.borrow(), "stopping");
        });
        Ok::<_, BoxError>(())
    })
```

| Item | Description |
|---|---|
| `per_worker_service(factory)` | Factory is `Fn(WorkerContext) -> Future<Output = Result<S, BoxError>>`; it must be `Send + Sync`, the future and `S` need not be `Send` |
| `WorkerContext::id()`, `workers()` | Worker index and count |
| `WorkerContext::shutdown()` | A `CancelToken` cancelled with the worker's listener |
| `WorkerContext::spawn_local(fut)` | Spawn a `!Send` task on this worker |
| `WorkerService::shutdown(self: Box<Self>)` | Awaited after the HTTP drain, in reverse start order; `()` implements it |

Startup is all-or-nothing: if any factory fails, every worker shuts down and
`run()` returns the error. `per_worker_service` requires `server.workers`;
without it `run()` fails rather than running the factory on the wrong runtime.

A runnable example is `examples/example-worker-udp`.

## More tools

- `WorkerSet` bean and `WorkerHealth`: aggregated worker lifecycle for health
  checks.
- `Mailboxes<M>`: messaging between workers.
- `WorkerHarness`: test worker code without starting a server.
- `r2e::r2e_prometheus::WorkerCollector`: per-worker metrics.

## See also

- Full guide: [docs/features/19-sharded-serving.md](https://github.com/plawn/r2e/blob/master/docs/features/19-sharded-serving.md)
- [Performance](./performance.md)
- [Serve Lifecycle](./serve-lifecycle.md)
