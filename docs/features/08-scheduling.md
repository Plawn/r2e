# Feature 8 — Scheduling

## Objective

Execute background tasks periodically (fixed interval or cron expression), with graceful shutdown via `CancellationToken`.

## Key Concepts

### Scheduler

The scheduled task manager. It collects tasks and then starts them as background Tokio tasks.

### ScheduledTask

An individual task with a name, a scheduling type (`Schedule`), and an async closure that receives the application state.

### Schedule

An enum defining when the task executes:
- `Every(Duration)` — fixed interval
- `EveryDelay { interval, initial_delay }` — interval with initial delay
- `Cron(String)` — cron expression

### CancellationToken

A token from `tokio-util` that allows graceful shutdown of scheduled tasks (typically when the server stops).

## Usage

### 1. Add the dependencies

```toml
[dependencies]
r2e-scheduler = { path = "../r2e-scheduler" }
tokio-util = { version = "0.7", features = ["rt"] }
```

### 2. Create a Scheduler and add tasks

```rust
use std::time::Duration;
use r2e_scheduler::{Scheduler, ScheduledTask, Schedule};
use tokio_util::sync::CancellationToken;

let cancel = CancellationToken::new();
let mut scheduler = Scheduler::new();

// Task executed every 30 seconds
scheduler.add_task(ScheduledTask {
    name: "user-count".to_string(),
    schedule: Schedule::Every(Duration::from_secs(30)),
    task: Box::new(|state: Services| {
        Box::pin(async move {
            let count = state.user_service.count().await;
            tracing::info!(count, "Nombre d'utilisateurs");
        })
    }),
});
```

### 3. Start the scheduler

```rust
// Start all tasks in the background
scheduler.start(services.clone(), cancel.clone());
```

Tasks run until the `CancellationToken` is cancelled.

### 4. Stop the scheduler

```rust
// When the application shuts down
cancel.cancel();
```

Typically placed after `AppBuilder::serve()`:

```rust
AppBuilder::new()
    .with_state(services)
    // ...
    .serve("0.0.0.0:3000")
    .await
    .unwrap();

// The server has stopped → stop the scheduler
cancel.cancel();
```

## Schedule types

### Schedule::Every

Executes the task at a fixed interval, immediately at startup then at each tick:

```rust
Schedule::Every(Duration::from_secs(60))  // Every 60 seconds
```

### Schedule::EveryDelay

Like `Every`, but with a delay before the first execution:

```rust
Schedule::EveryDelay {
    interval: Duration::from_secs(60),
    initial_delay: Duration::from_secs(10),  // Wait 10s before starting
}
```

### Schedule::Cron

Cron expression (6 fields: sec min hour day month weekday):

```rust
Schedule::Cron("0 */5 * * * *".to_string())  // Every 5 minutes
Schedule::Cron("0 0 * * * *".to_string())     // Every hour
Schedule::Cron("0 0 2 * * *".to_string())     // Every day at 2am
```

**Note**: the cron uses the `cron` crate with 6 fields (seconds first).

## State access

Each task receives a copy of the application state (`T: Clone`). This means tasks have access to services, the database pool, the event bus, etc.:

```rust
scheduler.add_task(ScheduledTask {
    name: "cleanup-expired-sessions".to_string(),
    schedule: Schedule::Every(Duration::from_secs(300)),
    task: Box::new(|state: Services| {
        Box::pin(async move {
            sqlx::query("DELETE FROM sessions WHERE expired_at < datetime('now')")
                .execute(&state.pool)
                .await
                .ok();
        })
    }),
});
```

## Logs

The scheduler automatically logs the start and stop of each task:

```
INFO task="user-count" "Scheduled task started"
DEBUG task="user-count" "Executing scheduled task"
INFO task="user-count" "Scheduled task stopped"
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
