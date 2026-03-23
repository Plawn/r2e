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
    // Run every 30 seconds (integer = seconds)
    #[scheduled(every = 30)]
    async fn count_users(&self) {
        let count = self.user_service.count().await;
        tracing::info!(count, "Scheduled user count");
    }

    // Run every 5 minutes (duration string)
    #[scheduled(every = "5m")]
    async fn sync_data(&self) {
        tracing::info!("Syncing data");
    }

    // Run on a cron schedule (every hour) — validated at compile time
    #[scheduled(cron = "0 0 * * * *")]
    async fn hourly_cleanup(&self) {
        tracing::info!("Running hourly cleanup");
    }

    // Run every 60 seconds, first execution after 10 second delay
    #[scheduled(every = "1m", initial_delay = "10s")]
    async fn delayed_task(&self) {
        tracing::info!("Delayed task executed");
    }
}
```

## Schedule types

`every` and `initial_delay` accept either an integer (interpreted as seconds) or a duration string with suffixes `ms`, `s`, `m`, `h`, `d`. Suffixes are combinable: `"1h30m"`, `"2m30s"`.

| Attribute | Description | Example |
|-----------|-------------|---------|
| `every = N` | Run every N seconds | `#[scheduled(every = 30)]` |
| `every = "dur"` | Run at a duration interval | `#[scheduled(every = "5m")]` |
| `every = .., initial_delay = ..` | Interval with initial delay | `#[scheduled(every = "1m", initial_delay = "10s")]` |
| `cron = "expr"` | Cron expression (6 fields, validated at compile time) | `#[scheduled(cron = "0 */5 * * * *")]` |

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

## SchedulerHandle

The `SchedulerHandle` is an Axum extractor that gives HTTP handlers access to the scheduler runtime. Use it to check scheduler status or trigger cancellation from an endpoint.

### Extracting SchedulerHandle

Add `SchedulerHandle` as a parameter to any handler method:

```rust
use r2e::r2e_scheduler::SchedulerHandle;

#[derive(Controller)]
#[controller(path = "/admin", state = AppState)]
pub struct AdminController {
    #[inject] some_service: SomeService,
}

#[routes]
impl AdminController {
    #[get("/scheduler/status")]
    async fn scheduler_status(&self, scheduler: SchedulerHandle) -> Json<bool> {
        Json(scheduler.is_cancelled())
    }

    #[post("/scheduler/stop")]
    async fn stop_scheduler(&self, scheduler: SchedulerHandle) -> StatusCode {
        scheduler.cancel();
        StatusCode::OK
    }
}
```

### SchedulerHandle methods

| Method | Return type | Description |
|--------|-------------|-------------|
| `is_cancelled()` | `bool` | Check if the scheduler has been cancelled |
| `cancel()` | `()` | Cancel the scheduler and all running tasks |
| `token()` | `CancellationToken` | Get the underlying `CancellationToken` |

> **Note:** `SchedulerHandle` requires the `Scheduler` plugin to be installed. If it is missing, extraction returns a `500 Internal Server Error` with a descriptive message.

## ScheduledJobRegistry

The `ScheduledJobRegistry` provides runtime introspection of all registered scheduled jobs. Unlike `SchedulerHandle` (which is an Axum extractor), the registry is a bean that you inject via `#[inject]` on a controller field.

### Injecting the registry

```rust
use r2e::r2e_scheduler::{ScheduledJobRegistry, ScheduledJobInfo};

#[derive(Controller)]
#[controller(path = "/admin", state = AppState)]
pub struct JobAdminController {
    #[inject] jobs: ScheduledJobRegistry,
}

#[routes]
impl JobAdminController {
    #[get("/jobs")]
    async fn list_jobs(&self) -> Json<Vec<ScheduledJobInfo>> {
        Json(self.jobs.list_jobs())
    }
}
```

### ScheduledJobInfo fields

Each entry returned by `list_jobs()` is a `ScheduledJobInfo` with:

| Field | Type | Description | Example value |
|-------|------|-------------|---------------|
| `name` | `String` | The name of the scheduled task | `"count_users"` |
| `schedule` | `String` | Human-readable schedule description | `"every 30s"`, `"every 60s (delay 10s)"`, `"cron: 0 */5 * * * *"` |

### ScheduledJobRegistry methods

| Method | Return type | Description |
|--------|-------------|-------------|
| `list_jobs()` | `Vec<ScheduledJobInfo>` | Returns a snapshot of all registered jobs |
| `register(info)` | `()` | Register a job (used internally by the scheduler) |

### Combining SchedulerHandle and ScheduledJobRegistry

You can use both together to build a full admin dashboard:

```rust
use r2e::r2e_scheduler::{SchedulerHandle, ScheduledJobRegistry, ScheduledJobInfo};

#[derive(Controller)]
#[controller(path = "/admin/scheduler", state = AppState)]
pub struct SchedulerAdminController {
    #[inject] jobs: ScheduledJobRegistry,
}

#[routes]
impl SchedulerAdminController {
    #[get("/jobs")]
    async fn list_jobs(&self) -> Json<Vec<ScheduledJobInfo>> {
        Json(self.jobs.list_jobs())
    }

    #[get("/status")]
    async fn status(&self, handle: SchedulerHandle) -> Json<serde_json::Value> {
        let jobs = self.jobs.list_jobs();
        Json(serde_json::json!({
            "cancelled": handle.is_cancelled(),
            "job_count": jobs.len(),
            "jobs": jobs.iter().map(|j| serde_json::json!({
                "name": j.name,
                "schedule": j.schedule,
            })).collect::<Vec<_>>(),
        }))
    }

    #[post("/cancel")]
    async fn cancel(&self, handle: SchedulerHandle) -> StatusCode {
        handle.cancel();
        StatusCode::OK
    }
}
```

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

    #[scheduled(every = "5m")]
    async fn refresh_stats(&self) {
        self.stats_service.refresh().await;
    }
}
```
