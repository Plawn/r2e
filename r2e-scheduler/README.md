# r2e-scheduler

Background task scheduler for R2E — interval, cron, and delayed task execution.

## Overview

Provides a scheduler plugin that auto-discovers `#[scheduled]` methods in controllers and runs them as background Tokio tasks. Tasks are lifecycle-managed via `CancellationToken` for clean shutdown.

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.1", features = ["scheduler"] }
```

## Setup

Install the `Scheduler` plugin **before** `build_state()`:

```rust
use r2e::r2e_scheduler::Scheduler;

AppBuilder::new()
    .plugin(Scheduler)
    .build_state::<AppState, _>()
    .await
    .register_controller::<ScheduledJobs>()
    .serve("0.0.0.0:3000")
    .await;
```

## Declarative scheduling

```rust
#[derive(Controller)]
#[controller(path = "/jobs", state = AppState)]
pub struct ScheduledJobs {
    #[inject] service: CleanupService,
}

#[routes]
impl ScheduledJobs {
    #[scheduled(every = 30)]                     // every 30 seconds
    async fn cleanup(&self) {
        self.service.cleanup_expired().await;
    }

    #[scheduled(every = 60, delay = 10)]         // first run after 10s, then every 60s
    async fn sync(&self) {
        self.service.sync_external().await;
    }

    #[scheduled(cron = "0 */5 * * * *")]         // cron: every 5 minutes
    async fn report(&self) {
        self.service.generate_report().await;
    }
}
```

## Schedule types

| Config | Syntax | Description |
|--------|--------|-------------|
| Interval | `every = 30` | Fixed interval in seconds |
| Interval + delay | `every = 60, delay = 10` | Initial delay before first run |
| Cron | `cron = "0 */5 * * * *"` | Standard cron expression |

## Lifecycle

1. `Scheduler` plugin creates a `CancellationToken` during `plugin()` phase
2. Tasks are collected from controllers during `register_controller()`
3. `serve()` starts all tasks as Tokio background tasks
4. On shutdown signal (Ctrl-C / SIGTERM), the `CancellationToken` is cancelled
5. All tasks receive the cancellation and stop gracefully

## Constraints

Controllers with `#[inject(identity)]` struct fields cannot be used for scheduling (no `StatefulConstruct`). Use parameter-level `#[inject(identity)]` on individual handler methods instead.

## License

Apache-2.0
