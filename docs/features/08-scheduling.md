# Feature 8 — Scheduling

## Objective

Execute background tasks periodically (fixed interval or cron expression), with graceful shutdown via `CancellationToken`.

## Key Concepts

### Scheduler plugin

The scheduled task manager. Installed with `.plugin(Scheduler)` **before** `build_state()`, it collects every `#[scheduled]` method of the registered controllers and starts them as background Tokio tasks.

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
r2e = { version = "...", features = ["scheduler"] }
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

AppBuilder::new()
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

## Logs

The scheduler automatically logs the start and stop of each task:

```
INFO task="count_users" "Scheduled task started"
DEBUG task="count_users" "Executing scheduled task"
INFO task="count_users" "Scheduled task stopped"
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
