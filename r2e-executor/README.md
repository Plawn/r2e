# r2e-executor

Managed task pool executor for R2E — injectable, bounded, with graceful shutdown.

## Overview

Provides a bounded, configurable Tokio task pool (a la J2EE `ManagedExecutorService`). Install as a plugin to make `PoolExecutor` available for `#[inject]` in any controller or bean.

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.1", features = ["executor"] }
```

### Setup

```rust
use r2e::prelude::*;
use r2e_executor::Executor;

AppBuilder::new()
    .plugin(Executor)
    .build_state()
    .await
    .register_controller::<MyController>()
    .serve("0.0.0.0:3000")
    .await;
```

### Submit work

```rust
#[controller(path = "/reports")]
pub struct ReportController {
    #[inject] executor: PoolExecutor,
}

#[routes]
impl ReportController {
    #[post("/")]
    async fn create(&self) -> Json<&'static str> {
        self.executor.submit_detached(async move {
            // long-running background work
        });
        Json("queued")
    }
}
```

## Configuration

```yaml
executor:
  max-concurrent: 32       # semaphore size (default: 32)
  queue-capacity: 1024     # max queued jobs (default: 1024)
  shutdown-timeout: 30s    # graceful drain timeout
```

## Key types

| Type | Description |
|------|-------------|
| `Executor` | `PreStatePlugin` — installs `PoolExecutor` into the bean graph |
| `PoolExecutor` | Injectable task pool — `submit`, `submit_detached`, `try_submit` |
| `ExecutorConfig` | Typed config from `executor.*` section |
| `JobHandle` | Handle to a submitted job — `.await` for the result, `abort()` to cancel |
| `ExecutorMetrics` | Snapshot of `queued` / `running` / `completed` / `rejected` counters (`PoolExecutor::metrics()`) |
| `RejectedError` | `submit`/`try_submit` failure — `QueueFull` (backpressure) or `Shutdown` |

`submit` and `try_submit` return `Result<JobHandle<T>, RejectedError>`; `try_submit`
applies queue-depth backpressure (rejects with `QueueFull` once
`queued + running >= max-concurrent + queue-capacity`), while `submit` always
queues. On shutdown the pool refuses new work and drains in-flight jobs, bounded
by `executor.shutdown-timeout`.

## Background execution helpers

Beyond direct `submit`, the executor feature adds two macro-driven helpers:

- **`#[async_exec]`** — on a `#[bean]` impl or a `#[routes]` controller (both
  work since W10), rewrites a method into a synchronous wrapper that submits the
  body to a `PoolExecutor` field and returns `Result<JobHandle<T>, RejectedError>`
  instead of `T`. The executor field defaults to `executor`; override with
  `#[async_exec(executor = "io_pool")]`.
- **`#[derive(BackgroundService)]`** — generates a `ServiceComponent` from
  `#[inject]` / `#[config]` fields (resolved from the bean graph by type). Supply
  an `async fn run(&self, CancellationToken)`, then register with
  `AppBuilder::spawn_service::<MyService>()`; its handle is awaited on graceful
  shutdown.

See `docs/claude/executor.md` for full examples and the compile-time constraints.

## License

Apache-2.0
