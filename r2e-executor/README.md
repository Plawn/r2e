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
| `JobHandle` | Handle to a submitted job (cancel, await result) |

## License

Apache-2.0
