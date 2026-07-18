# Scheduling

R2E provides declarative background task scheduling with interval, cron, and delayed execution.

## Setup

Enable the scheduler feature and install the `Scheduler` plugin:

```toml
r2e = { version = "0.1", features = ["scheduler"] }   # pulls in "executor"
```

```rust
use r2e::r2e_scheduler::Scheduler;
use r2e::r2e_executor::Executor;

AppBuilder::new()
    .plugin(Executor)                   // required by Scheduler (ticks run on the pool)
    .plugin(Scheduler)                  // MUST be before build_state()
    .build_state()
    .await
    .register_controller::<ScheduledJobs>()
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

Both plugins must be installed **before** `build_state()`. The `Scheduler` provides a `CancellationToken` and a `ScheduledJobRegistry` to the bean graph and **requires the `Executor` plugin**: it declares `type LateDeps = (PoolExecutor,)`, so `.plugin(Scheduler)` without a `PoolExecutor` in the graph fails at `build_state()` with a guided "missing `.provide::<PoolExecutor>()` / `.register::<PoolExecutor>()`" error. Order between the two plugins does not matter (`LateDeps` are checked against the final provision list), and the `scheduler` feature pulls in `executor`.

### Configuration (`scheduler.*`)

The plugin reads an optional `scheduler.*` YAML section (`SchedulerConfig`, `CONFIG_PREFIX = "scheduler"`). All keys are optional:

```yaml
scheduler:
  enabled: true            # standard <prefix>.enabled gate; when false, tasks don't start (beans still provided)
  executor: dedicated      # "shared" (default) or "dedicated"
  max-concurrent: 8        # dedicated pool only
  queue-capacity: 256      # dedicated pool only
  shutdown-timeout: 10s    # dedicated pool only
```

By default ticks run on the shared `PoolExecutor`. Set `executor: dedicated` to give scheduled work a private pool (sized by the keys above) so it never contends with other background jobs.

## Declaring scheduled tasks

Use `#[scheduled]` on controller methods:

```rust
#[controller]
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
| `name = ".."` | Override the task name (default `<Controller>_<method>`) | `#[scheduled(every = 30, name = "user_count")]` |
| `overlap = ".."` | Self-overlap policy: `"skip"` (default) or `"concurrent"` | `#[scheduled(every = "50ms", overlap = "concurrent")]` |
| `skip_if = ".."` | Names a `&self -> bool` predicate that suppresses a tick | `#[scheduled(every = "5m", skip_if = "maintenance_mode")]` |

### Overlap policy and skip predicate

By default a task uses `overlap = "skip"`: if a tick is still running when the next one is due, that tick is skipped. Use `overlap = "concurrent"` to let ticks run in parallel.

`skip_if = "method"` names a plain `&self` method (sync or async) on the **same impl block** returning `bool`. It is evaluated before every tick; `true` suppresses the body and counts in `ScheduledJobInfo::skip_count` (Quarkus `skipExecutionIf`):

```rust
#[routes]
impl ScheduledJobs {
    fn maintenance_mode(&self) -> bool {
        // gate on some shared state
        false
    }

    #[scheduled(every = "5m", skip_if = "maintenance_mode")]
    async fn sync(&self) {
        // ...
    }
}
```

### Cron expression format

Six fields: `second minute hour day_of_month month day_of_week`

```
0 */5 * * * *      — every 5 minutes
0 0 * * * *        — every hour
0 0 0 * * *        — every day at midnight
0 30 9 * * MON-FRI — weekdays at 9:30 AM
```

## Requirements

- Scheduled methods run on the controller core (built from the bean graph via `ContextConstruct`) and cannot access request-scoped fields — reading `#[inject(identity)]` / `#[inject(request)]` inside a scheduled method is a compile error. `ContextConstruct` is generated for **every** controller core (identity and request-scoped fields are stripped onto the per-request façade), so a controller may freely combine struct-level identity for its authenticated endpoints with `#[scheduled]` tasks. Scheduled methods use only core (`#[inject]` / `#[config]`) fields.
- The `Scheduler` **and `Executor`** plugins must be installed before `build_state()` (the Scheduler runs its ticks on the executor pool)
- Scheduled methods take `&self` only (no additional parameters)

## How it works

1. `Scheduler` plugin creates a `CancellationToken` and defers setup
2. `build_state()` provides the token to the bean graph and verifies the `PoolExecutor` dependency (`Scheduler::LateDeps`)
3. `register_controller::<ScheduledJobs>()` collects scheduled task definitions
4. `serve()` starts each schedule loop; every tick body is submitted to the shared `PoolExecutor` and the loop awaits it before the next tick
5. On shutdown (Ctrl-C / SIGTERM), the `CancellationToken` is cancelled and in-flight ticks drain via the pool (`executor.shutdown-timeout`)

Because ticks run as pool jobs, a panicking tick is contained and logged (its schedule loop keeps running), scheduled work is bounded by `executor.max-concurrent`, and it appears in `ExecutorMetrics`.

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

#[controller(path = "/admin")]
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
| `pause(name).await` | `bool` | Pause a job by name (keeps advancing its cadence but never fires). `false` if unknown |
| `resume(name).await` | `bool` | Resume a paused job by name. `false` if unknown |
| `trigger_now(name).await` | `bool` | Fire a job once, immediately and out of band (allowed even when paused). `false` if unknown or a `skip`-overlap tick is already in flight |

> **Note:** `SchedulerHandle` requires the `Scheduler` plugin to be installed. If it is missing, extraction returns a `500 Internal Server Error` with a descriptive message.

## ScheduledJobRegistry

The `ScheduledJobRegistry` provides runtime introspection of all registered scheduled jobs. Unlike `SchedulerHandle` (which is an Axum extractor), the registry is a bean that you inject via `#[inject]` on a controller field.

### Injecting the registry

```rust
use r2e::r2e_scheduler::{ScheduledJobRegistry, ScheduledJobInfo};

#[controller(path = "/admin")]
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

The metadata (`name`, `schedule`) is fixed at registration; the remaining fields carry live runtime stats updated by the driver as the job runs.

| Field | Type | Description | Example value |
|-------|------|-------------|---------------|
| `name` | `String` | The name of the scheduled task | `"count_users"` |
| `schedule` | `String` | Human-readable schedule description | `"every 30s"`, `"every 60s (delay 10s)"`, `"cron: 0 */5 * * * *"` |
| `last_run` | `Option<DateTime<Utc>>` | Wall-clock time the job most recently fired | `None` until first run |
| `last_duration` | `Option<Duration>` | Wall duration of the most recent completed tick | |
| `next_run` | `Option<DateTime<Utc>>` | Wall-clock time the job is next expected to fire (`None` for a spent cron) | |
| `run_count` | `u64` | Number of ticks whose body actually ran | `42` |
| `skip_count` | `u64` | Number of ticks suppressed by the job's `skip_if` predicate | `3` |
| `panic_count` | `u64` | Number of ticks that panicked (contained by the pool) | `0` |
| `paused` | `bool` | Whether the job is currently paused | `false` |

### ScheduledJobRegistry methods

| Method | Return type | Description |
|--------|-------------|-------------|
| `list_jobs()` | `Vec<ScheduledJobInfo>` | Returns a snapshot of all registered jobs |
| `job(name)` | `Option<ScheduledJobInfo>` | Snapshot of a single job by name |
| `register(info)` | `()` | Register a job (used internally by the scheduler) |

### Combining SchedulerHandle and ScheduledJobRegistry

You can use both together to build a full admin dashboard:

```rust
use r2e::r2e_scheduler::{SchedulerHandle, ScheduledJobRegistry, ScheduledJobInfo};

#[controller(path = "/admin/scheduler")]
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

## Bean scheduled tasks

`#[scheduled]` also works on `#[bean]` methods — no controller needed. The `#[bean]` macro generates the task source and an `after_register` hook, so `.register::<T>()` alone collects the tasks at `build_state()`:

```rust
#[derive(Clone)]
pub struct CleanupBean {
    store: Store,
}

#[bean]
impl CleanupBean {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    #[scheduled(every = "1h", name = "cleanup")]
    async fn tick(&self) {
        self.store.purge_expired().await;
    }
}
```

```rust
AppBuilder::new()
    .plugin(Executor)
    .plugin(Scheduler)
    .register::<CleanupBean>()
    .build_state()
    .await
    // ...
```

Bean scheduled methods take `&self` and support the same `every` / `cron` / `initial_delay` / `overlap` / `skip_if` attributes as controller methods.

## Mixed controllers

A controller can have both HTTP routes and scheduled tasks:

```rust
#[controller(path = "/stats")]
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
