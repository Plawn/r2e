# Scheduling

R2E provides declarative background task scheduling with interval, cron, and delayed execution.

## Setup

Enable the scheduler feature and install the `Scheduler` plugin:

```toml
r2e = { version = "0.1", features = ["scheduler"] }
```

```rust
use r2e::r2e_scheduler::Scheduler;

AppBuilder::new()
    .plugin(Scheduler)                  // MUST be before build_state()
    .build_state::<AppState, _, _>()
    .await
    .register_controller::<ScheduledJobs>()
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

The `Scheduler` plugin must be installed **before** `build_state()` because it provides a `CancellationToken` to the bean graph.

## Declaring scheduled tasks

Use `#[scheduled]` on controller methods:

```rust
#[derive(Controller)]
#[controller(state = AppState)]
pub struct ScheduledJobs {
    #[inject] user_service: UserService,
}

#[routes]
impl ScheduledJobs {
    // Run every 30 seconds
    #[scheduled(every = 30)]
    async fn count_users(&self) {
        let count = self.user_service.count().await;
        tracing::info!(count, "Scheduled user count");
    }

    // Run on a cron schedule (every hour)
    #[scheduled(cron = "0 0 * * * *")]
    async fn hourly_cleanup(&self) {
        tracing::info!("Running hourly cleanup");
    }

    // Run every 60 seconds, first execution after 10 second delay
    #[scheduled(every = 60, delay = 10)]
    async fn delayed_task(&self) {
        tracing::info!("Delayed task executed");
    }
}
```

## Schedule types

| Attribute | Description | Example |
|-----------|-------------|---------|
| `every = N` | Run every N seconds | `#[scheduled(every = 30)]` |
| `every = N, delay = D` | Every N seconds, first run after D seconds | `#[scheduled(every = 60, delay = 10)]` |
| `cron = "expr"` | Cron expression (6 fields) | `#[scheduled(cron = "0 */5 * * * *")]` |

### Cron expression format

Six fields: `second minute hour day_of_month month day_of_week`

```
0 */5 * * * *      — every 5 minutes
0 0 * * * *        — every hour
0 0 0 * * *        — every day at midnight
0 30 9 * * MON-FRI — weekdays at 9:30 AM
```

## Requirements

- Scheduled controller must **not** have struct-level `#[inject(identity)]` fields (needs `StatefulConstruct`)
- The `Scheduler` plugin must be installed before `build_state()`
- Scheduled methods take `&self` only (no additional parameters)

## How it works

1. `Scheduler` plugin creates a `CancellationToken` and defers setup
2. `build_state()` provides the token to the bean graph
3. `register_controller::<ScheduledJobs>()` collects scheduled task definitions
4. `serve()` starts all scheduled tasks as Tokio tasks
5. On shutdown (Ctrl-C / SIGTERM), the `CancellationToken` is cancelled, stopping all tasks

## Error handling in scheduled tasks

Scheduled methods can return `Result`:

```rust
#[scheduled(every = 60)]
async fn cleanup(&self) -> Result<(), Box<dyn std::error::Error>> {
    self.service.cleanup().await?;
    Ok(())
}
```

Errors are logged but don't stop the scheduler — the task runs again at the next interval.

## Mixed controllers

A controller can have both HTTP routes and scheduled tasks:

```rust
#[derive(Controller)]
#[controller(path = "/stats", state = AppState)]
pub struct StatsController {
    #[inject] stats_service: StatsService,
}

#[routes]
impl StatsController {
    #[get("/")]
    async fn get_stats(&self) -> Json<Stats> {
        Json(self.stats_service.current().await)
    }

    #[scheduled(every = 300)]
    async fn refresh_stats(&self) {
        self.stats_service.refresh().await;
    }
}
```
