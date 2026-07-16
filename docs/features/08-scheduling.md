# Feature 8 — Scheduling

## Objective

Execute background tasks periodically (fixed interval or cron expression), with graceful shutdown via `CancellationToken`.

## Key Concepts

### Scheduler plugin

The scheduled task manager. Installed with `.plugin(Scheduler)` **before** `build_state()`, it collects every `#[scheduled]` method of the registered controllers and starts each as a background schedule loop. **The Scheduler requires the Executor plugin** — it declares `type LateDeps = (PoolExecutor,)`, so a build with `.plugin(Scheduler)` but no `PoolExecutor` in the graph fails at `build_state()` with the guided "missing `.provide::<PoolExecutor>()` / `.register::<PoolExecutor>()`" error. Because `LateDeps` are checked against the final provision list, the order between `.plugin(Executor)` and `.plugin(Scheduler)` does not matter. The `scheduler` feature pulls in `executor`.

### `#[scheduled]`

An attribute on a controller method that turns it into a scheduled task. The controller core is built from the resolved bean graph (`ContextConstruct::from_context`), so `#[inject]` fields are available inside the task — but no request-scoped data (there is no HTTP request).

### ScheduleConfig

An enum defining when the task executes:
- `Interval(Duration)` — fixed interval
- `IntervalWithDelay { interval, initial_delay }` — interval with initial delay
- `Cron(String)` — cron expression

### CancellationToken

A token from `tokio-util` that allows graceful shutdown of scheduled tasks (typically when the server stops). The scheduler manages it for you; `SchedulerHandle` exposes it if you need manual control.

## Usage

### 1. Add the dependencies

```toml
[dependencies]
r2e = { version = "...", features = ["scheduler"] }   # pulls in "executor"
```

### 2. Declare scheduled methods on a controller

A scheduled-only controller needs no route path — bare `#[controller]` is enough. `#[inject]` fields are resolved from the bean graph by type:

```rust
use r2e::prelude::*;

#[controller]
pub struct ScheduledJobs {
    #[inject]
    user_service: UserService,
}

#[routes]
impl ScheduledJobs {
    // Task executed every 30 seconds
    #[scheduled(every = 30)]
    async fn count_users(&self) {
        let count = self.user_service.count().await;
        tracing::info!(count, "Scheduled user count");
    }
}
```

### 3. Install the plugin and register the controller

The `Scheduler` plugin goes on the builder **before** `build_state()`; the controller is registered **after**:

```rust
use r2e::prelude::*;
use r2e::r2e_scheduler::Scheduler;
use r2e::r2e_executor::Executor;

AppBuilder::new()
    .plugin(Executor)                       // required by Scheduler (ticks run on the pool)
    .plugin(Scheduler)
    .register::<UserService>()
    .build_state()
    .await
    .register_controller::<ScheduledJobs>()
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

Tasks run until the server shuts down; the scheduler then cancels its `CancellationToken` and every task stops gracefully.

### 4. Manual control (optional)

`SchedulerHandle` can be extracted as a handler parameter to inspect or cancel the scheduler at runtime:

```rust
#[get("/scheduler/status")]
async fn status(&self, scheduler: SchedulerHandle) -> Json<bool> {
    Json(scheduler.is_cancelled())
}
```

The `Scheduler` plugin also provides a `ScheduledJobRegistry` bean (`#[inject] jobs: ScheduledJobRegistry`) to list the registered jobs (e.g., for an admin endpoint).

## Schedule types

### Fixed interval

Executes the task at a fixed interval, immediately at startup then at each tick:

```rust
#[scheduled(every = 60)]  // Every 60 seconds
```

### Interval with initial delay

Like `every`, but with a delay before the first execution:

```rust
#[scheduled(every = 60, initial_delay = 10)]  // Wait 10s before starting
```

### Cron

Cron expression (6 fields: sec min hour day month weekday):

```rust
#[scheduled(cron = "0 */5 * * * *")]  // Every 5 minutes
#[scheduled(cron = "0 0 * * * *")]    // Every hour
#[scheduled(cron = "0 0 2 * * *")]    // Every day at 2am
```

**Note**: the cron uses the `cron` crate with 6 fields (seconds first).

## State access

Each task run builds a fresh controller core from the resolved bean graph via `ContextConstruct::from_context` — every `#[inject]` field resolves by type. This means tasks have access to services, the database pool, the event bus, etc.:

```rust
#[controller]
pub struct CleanupJobs {
    #[inject]
    pool: sqlx::SqlitePool,
}

#[routes]
impl CleanupJobs {
    #[scheduled(every = 300)]
    async fn cleanup_expired_sessions(&self) {
        sqlx::query("DELETE FROM sessions WHERE expired_at < datetime('now')")
            .execute(&self.pool)
            .await
            .ok();
    }
}
```

A scheduled method may return `()` or `Result<(), E>` — errors are logged, not fatal.

## Execution model

All schedules share a **single driver task**: one `rt::spawn`ed loop owns a
min-heap of next-fire deadlines for every task (there is no longer one Tokio task
per schedule). When the earliest deadline is reached, the driver submits the due
tick bodies to the shared `PoolExecutor` (the Quarkus model — `executor.submit(...)`)
and tracks the resulting `JobHandle`s in a `FuturesUnordered`. A job is re-armed
onto the heap only when its own tick completes. This means:

- **Non-overlap is preserved** — a job is either waiting in the heap or in flight,
  never both, so a task never overlaps with its own next tick
  (`MissedTickBehavior::Skip` semantics, computed against the job's fixed cadence).
- **Jobs still run concurrently with each other** — the driver never awaits a tick
  inline, so a slow tick on one schedule does not delay the ticks of any other.
- **In-flight ticks drain on shutdown** — they are pool jobs covered by
  `executor.shutdown-timeout` (`PoolExecutor::shutdown_graceful`). The driver breaks
  on cancellation without aborting them.
- **A panicking tick no longer kills the driver** — the panic is contained in the
  pool job, logged, and the job is re-armed as usual.
- **Scheduled work is bounded and observable** — it counts against
  `executor.max-concurrent` and shows up in `ExecutorMetrics`
  (running / queued / completed / rejected), alongside other submitted jobs. When the
  pool is shut down, the driver stops (nothing can run anymore).

## Overlap policy

By default a job never runs concurrently with itself (`skip`). Opt into
overlapping ticks per task:

```rust
#[scheduled(every = "50ms", overlap = "concurrent")]   // also valid with cron
async fn poll(&self) { /* may overlap under sustained load */ }
```

- **`skip`** (default) — re-arm on completion; a fire that comes due while the
  previous tick is still running is skipped, and the schedule is advanced so
  cadence is preserved. The job is either in the heap or in flight, never both.
- **`concurrent`** — re-arm at *fire* time (the next deadline is pushed back
  before the tick is even submitted; its completion does not re-arm), so a slow
  tick never holds back the following one. Ticks may pile up. Interval cadence
  stays anchored; cron recomputes the next fire when the job fires.

Dynamic tasks use the builder form:
`ScheduledTaskDef::new(..).with_overlap(OverlapPolicy::Concurrent)`.

## Dedicated pool (config)

By default scheduled ticks share the app-wide `PoolExecutor`. The `scheduler.*`
section can give the scheduler a **private** pool so its work never contends with
other background jobs:

```yaml
scheduler:
  enabled: true            # standard <prefix>.enabled gate (false skips starting tasks)
  executor: dedicated      # "shared" (default) | "dedicated"
  max-concurrent: 8        # dedicated only — mirrors executor.*
  queue-capacity: 256      # dedicated only
  shutdown-timeout: 10s    # dedicated only
```

In dedicated mode a private `PoolExecutor` is built from the sizing keys and
drained gracefully on shutdown. `PoolExecutor` remains a hard `LateDeps`
requirement even in dedicated mode (a type-level requirement cannot be
config-conditional) — the shared pool is simply not used to run ticks. An
unrecognized `executor` value panics at boot. Setting `scheduler.enabled = false`
skips the plugin's post-state effects (starting tasks) while keeping its provided
beans (`CancellationToken`, `ScheduledJobRegistry`) in the graph.

## Runtime control and stats

Extract a `SchedulerHandle` as a handler parameter (or build one paired with the
driver via `SchedulerHandle::channel(token)` when driving `start_jobs` manually)
to control jobs at runtime by name — each call returns `bool` (`false` for an
unknown job, a handle with no driver, or a `skip` job already running):

```rust
scheduler.pause("count_users").await;      // stop firing; cadence still advances
scheduler.resume("count_users").await;     // start firing again
scheduler.trigger_now("count_users").await;// fire once, out of band (even if paused)
```

A paused job never submits; a `trigger_now` tick fires out of band, never re-arms,
and leaves the regular schedule untouched. `ScheduledJobRegistry` exposes live
stats per job (`list_jobs()` / `job(name)` → `ScheduledJobInfo`): `last_run` and
`next_run` (`chrono::DateTime<Utc>`), `last_duration`, `run_count`, `panic_count`,
and `paused`.

## Logs

The scheduler logs the driver lifecycle (with the job count) and any per-tick
panics or task errors:

```
INFO  count=3 "Scheduler driver started"
ERROR task="count_users" "Scheduled tick panicked"
INFO  count=3 "Scheduler driver stopped"
```

## Validation criteria

Start the application:

```bash
cargo run -p example-app
```

In the logs, every 30 seconds:

```
INFO count=2 "Scheduled user count"
```

On shutdown (Ctrl+C), the scheduler stops gracefully via the `CancellationToken`.
