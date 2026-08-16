# Scheduling

R2E provides declarative background task scheduling with interval, cron, and delayed execution.

## Setup

Enable the scheduler feature and install the `Scheduler` plugin:

```toml
r2e = { version = "0.3", features = ["scheduler"] }   # pulls in "executor"
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

Both plugins must be installed **before** `build_state()`. The `Scheduler` provides a `CancellationToken` and a `ScheduledJobRegistry` to the bean graph and **requires the `Executor` plugin**: it declares `type Deps = (PoolExecutor,)`, so `.plugin(Scheduler)` without a `PoolExecutor` in the graph fails at `build_state()` with a guided "missing `.provide::<PoolExecutor>()` / `.register::<PoolExecutor>()`" error. Order between the two plugins does not matter (`Deps` are checked against the final provision list), and the `scheduler` feature pulls in `executor`.

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
2. `build_state()` provides the token to the bean graph and verifies the `PoolExecutor` dependency (`Scheduler::Deps`)
3. `register_controller::<ScheduledJobs>()` collects scheduled task definitions
4. `serve()` starts **one** driver task for all schedules: a min-heap of next-fire times. The driver sleeps until the earliest deadline, then submits every due tick to the shared `PoolExecutor` and goes back to sleep — there is no task, and no loop, per schedule
5. Re-arming depends on the job's overlap policy: `Concurrent` schedules the next fire *before* submitting the tick (ticks may overlap), `Skip` (the default) re-arms only when its own tick completes, so a slow tick silently skips the cadence fires it overran
6. On shutdown (Ctrl-C / SIGTERM), the `CancellationToken` is cancelled: the driver stops arming, waits out the ticks already in flight, and they drain via the pool (`executor.shutdown-timeout`)

Because ticks run as pool jobs, a panicking tick body is contained and logged (the driver and every other job keep running), scheduled work is bounded by `executor.max-concurrent`, and it appears in `ExecutorMetrics`. A panic while *constructing* a tick — the closure body before its first `await` — is caught by the driver itself and disables that one job (see the contract below).

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
| `pause(name).await` | `bool` | Pause a job by name (keeps advancing its cadence but never fires). `false` if the name is unknown, or the scheduler is shutting down / has no driver |
| `resume(name).await` | `bool` | Resume a paused job by name; `true` iff the job can fire again (see the resume contract below). `false` if the name is unknown, the schedule can never fire again (spent cron, overflowed interval), or the scheduler is shutting down / has no driver |
| `trigger_now(name).await` | `bool` | Fire a job once, immediately and out of band (allowed even when paused). `false` if the name is unknown, a `skip`-overlap tick is already in flight, the executor pool is closed (answered first, then the driver stops), the tick factory panicked (contained; the job is disabled), or the scheduler is shutting down / has no driver (a `trigger_now` can never start a tick once shutdown began) |

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
| `next_run` | `Option<DateTime<Utc>>` | The deadline the job currently holds — `Some` iff a live driver holds one (for a *paused* job, when it *would* fire were it resumed: the cadence advances, the fire is skipped). `None` whenever it holds no deadline: a spent cron, a next fire that overflows the monotonic clock (an absurd interval or `initial_delay`, logged at `WARN`), a job disabled by a tick-factory panic that consumed its deadline, a `Skip` job while its tick is running, a deadline consumed by a submission that never ran, and every job once the driver has stopped | |
| `run_count` | `u64` | Number of ticks whose body actually ran | `42` |
| `skip_count` | `u64` | Number of ticks suppressed by the job's `skip_if` predicate | `3` |
| `panic_count` | `u64` | Ticks whose **body** panicked (contained by the pool, schedule untouched) plus tick **factory** panics — the user closure raising while the driver builds the tick, which also disables the job | `0` |
| `paused` | `bool` | Whether the job is currently paused: explicitly, or automatically after a tick-factory panic | `false` |

A tick-factory panic disables the job because a construction failure repeats on
every fire: the job is paused, `panic_count` goes up, and other jobs — and the
driver itself — keep running. `resume` is the way back, under one contract that
holds on every path:

> A paused job — by `pause`, or disabled by a tick-factory panic — never fires.
> `resume` replies `true` **iff the job can fire again, as far as the driver can
> tell when the command is handled**: it keeps its already-scheduled deadline
> when one is still armed; when one of its ticks is in flight and due to re-arm
> the job on completion, it reports whether the schedule still yields a next
> occurrence; otherwise it re-arms from now (interval: now + period; cron: the
> next matching slot) and reports the outcome. A schedule that can never fire
> again — spent or overflowed — stays unarmed and `resume` replies `false`.

So an ordinary pause → resume never double-fires a job (the cadence kept
advancing, the deadline is still there), a `Concurrent` job disabled by a
factory panic resumes onto the deadline it had already scheduled, a `Skip` job
disabled the same way is re-armed one period from the resume, and a job pinned
to a cron with no upcoming occurrence answers `false`.

The in-flight case is the one place the reply is a *snapshot* rather than a
guarantee: a cron's final slot can pass between the reply and the tick's
completion, leaving the job unarmed after a `true`. No implementation can
promise the future here — but the end state is always honest, and `next_run` is
the authority on it: it is `Some` **exactly** when a live driver holds a deadline
for the job. Read it as the deadline, not as the fire — a **paused** job keeps
publishing the instant it *would* fire were it resumed, because its cadence goes
on advancing and each deadline is re-armed instead of submitted. It reads `None`
the whole time a `Skip` tick runs (the deadline that fired is spent; completion
publishes the next one), for a spent or unrepresentable schedule, for a deadline
consumed by a submission that never ran — and for every job once the driver has
stopped, cancelled or halted by a closed pool: the driver clears what it
published on the way out, because nothing is left that could fire it.

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
