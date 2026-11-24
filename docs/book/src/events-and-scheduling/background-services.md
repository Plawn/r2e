# Executor and Background Services

The `r2e-executor` crate gives an app a managed task pool, `PoolExecutor`, with
a concurrency cap, a bounded queue and a graceful drain at shutdown. On top of
it, `#[async_exec]` runs a method on the pool, and
`#[derive(BackgroundService)]` declares a long-running worker tied to the app's
lifecycle.

## Setup

```toml
r2e = { version = "0.15", features = ["executor"] }
```

`executor` is part of `full`, and the `scheduler` feature turns it on as well.

```rust
use r2e::r2e_executor::{Executor, PoolExecutor, RejectedError};

AppBuilder::new()
    .plugin(Executor)          // provides PoolExecutor as a bean
    .load_config::<()>()       // the plugin reads executor.*
    .build_state()
    .await
```

## Configuration (`executor.*`)

```yaml
executor:
  max-concurrent: 32     # tasks running at once
  queue-capacity: 1024   # pending tasks before rejection
  shutdown-timeout: 30s  # drain budget; 0 = abort on shutdown
```

| Key | Default | Meaning |
|---|---|---|
| `executor.max-concurrent` | `32` | Running cap |
| `executor.queue-capacity` | `1024` | Waiting tasks before `QueueFull` |
| `executor.shutdown-timeout` | `30s` | How long shutdown waits for running tasks |

## `PoolExecutor`

Inject it like any bean (`#[inject] executor: PoolExecutor`):

```rust
// Err(RejectedError::Shutdown) once the pool is closed.
let handle = executor.submit(async { 21 + 21 }).expect("pool running");
let value: u32 = handle.await.expect("task ok");

// Bounded: rejects when the queue is full.
match executor.try_submit(async { /* ... */ }) {
    Ok(handle) => { /* JobHandle<T> */ }
    Err(RejectedError::QueueFull) => { /* backpressure: return 503 */ }
    Err(RejectedError::Shutdown) => { /* pool closed */ }
}

// Fire and forget.
executor.submit_detached(async move { /* ... */ });

// Live counters: running, queued, completed, rejected.
let metrics = executor.metrics();
```

## `#[async_exec]`

`#[async_exec]` turns an `async fn` on a `#[bean]` impl (or a controller) into a
method that submits its body to the pool. The generated method takes the same
arguments, is **not** `async`, and returns `Result<JobHandle<T>, RejectedError>`:

```rust
#[derive(Clone)]
pub struct ReportService {
    executor: PoolExecutor,
}

#[bean]
impl ReportService {
    pub fn new(executor: PoolExecutor) -> Self {
        Self { executor }
    }

    #[async_exec] // uses the `executor` field
    async fn generate_pdf(&self, id: u64) -> Vec<u8> {
        format!("PDF #{id}").into_bytes()
    }
}

// In a handler:
let job = self.reports.generate_pdf(42)?;    // returns at once
let bytes = job.await?;                       // or keep the handle for later
```

Use `#[async_exec(executor = "io_pool")]` to name another field. The attribute
cannot be combined with a route verb, `#[scheduled]`, `#[consumer]`,
`#[post_construct]` or a method-level `#[intercept]`.

## Background services

A background service is a struct with `#[inject]` and `#[config]` fields and an
`async fn run(&self, shutdown: CancelToken)` method:

```rust
use r2e::r2e_executor::{Executor, PoolExecutor};
use r2e::rt::{self, CancelToken};
use std::time::Duration;

#[derive(BackgroundService, Clone)]
pub struct EmailWorker {
    #[inject] executor: PoolExecutor,
    #[inject] mailer: Mailer,
    #[config("email.batch_size")] batch_size: u64,
}

impl EmailWorker {
    async fn run(&self, shutdown: CancelToken) {
        let mut interval = rt::interval(Duration::from_secs(60));
        loop {
            rt::select! {
                _ = shutdown.cancelled() => break,
                _ = interval.tick() => {
                    let mailer = self.mailer.clone();
                    self.executor.submit_detached(async move {
                        let _ = mailer.flush().await;
                    });
                }
            }
        }
    }
}

AppBuilder::new()
    .plugin(Executor)
    .build_state()
    .await
    .spawn_service::<EmailWorker>()
    .serve_auto()
    .await?;
```

A missing `#[inject]` type is a compile error at `spawn_service`. Shutdown
cancels the token, and the app waits for `run` to return (bounded by
`shutdown_grace_period`, see [Serve Lifecycle](../advanced/serve-lifecycle.md)).

### Service options

| Attribute | Effect |
|---|---|
| `#[service(stop = "after_drain")]` | Stop after the HTTP drain instead of before it. Use it for sinks fed by request handlers (audit log, outbox flusher). |
| `#[service(stop = "after_drain", order = N)]` | Order among after-drain services (ascending) |
| `#[service(enabled = "field")]` | Skip `run()` when the `bool` field (usually a `#[config]`) is `false`. The service is still built and its config still validated. |

### Turning all services off

```yaml
# application-test.yaml
services:
  enabled: false
```

This keeps every background service from running, which is handy in tests. It
is an ordinary config key, so `override_config_value("services.enabled", false)`
works too.

### Workers from other crates: `#[producer(start)]`

When the worker type has a plain constructor and no DI attributes,
`#[producer(start)]` builds it from beans and also runs it as a service, with no
wrapper struct. See `docs/claude/executor.md` for the full example.

## Choosing a primitive

| Goal | Use |
|---|---|
| Fire-and-forget side task from a handler | `executor.submit_detached(..)` |
| Slow task whose result is awaited later | `#[async_exec]` |
| Periodic or event-driven worker | `#[derive(BackgroundService)]` + `spawn_service` |
| Cron or interval schedule | `#[scheduled]` (its ticks run on this pool) |
| Schedules known only at runtime | [Dynamic Scheduled Tasks](./dynamic-scheduled-tasks.md) |

## Testing

Background services run in `TestApp` like in production. Turn them off with
`services.enabled: false` in `application-test.yaml` when a test should not
start them. `examples/example-executor` shows all three primitives behind HTTP
endpoints.

## See also

- Internals: [docs/claude/executor.md](https://github.com/plawn/r2e/blob/master/docs/claude/executor.md)
- [Scheduling](./scheduling.md)
- [Serve Lifecycle](../advanced/serve-lifecycle.md)
