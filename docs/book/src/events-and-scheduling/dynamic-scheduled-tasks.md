# Dynamic Scheduled Tasks

`#[scheduled]` covers tasks you know at compile time. Some apps need a task set
that is only known at runtime: one sync job per configured source, per tenant or
per feed. `AppBuilderSchedulerExt` registers such tasks on the same scheduler,
so they share the lifecycle of `#[scheduled]` tasks: started at boot, listed in
`ScheduledJobRegistry`, run on the executor pool and stopped on shutdown.

## Setup

```toml
r2e = { version = "0.15", features = ["scheduler"] }
```

`scheduler` is part of `full` and turns on `executor`. Everything below comes
from `use r2e::prelude::*`, except the executor plugin:

```rust
use r2e::prelude::*;
use r2e::r2e_executor::Executor;
```

Two rules:

- Install **`.plugin(Executor)` and `.plugin(Scheduler)` before `build_state()`**.
  Without `Scheduler`, `schedule_task` panics with a clear message. Without
  `Executor`, `build_state()` fails with an error naming the missing
  `PoolExecutor`.
- **Register tasks before `serve()`.** The registry is drained once at boot;
  tasks added later never start.

## One task per configured source

```rust
let app = AppBuilder::new()
    .plugin(Executor)
    .plugin(Scheduler)
    .provide(sync_service.clone())
    .build_state()
    .await;

let app = sources.iter().fold(app, |app, source| {
    let svc = sync_service.clone();
    let source = source.clone();
    app.schedule_task(ScheduledTaskDef::new(
        format!("sync_{}", source.name),
        source.schedule.clone(),     // ScheduleConfig
        svc,                         // state, cloned into each tick
        move |svc| {
            let source = source.clone();
            async move { svc.sync(&source).await }
        },
    ))
});

app.serve("0.0.0.0:3000").await?;
```

`schedule_tasks(iter)` registers a batch at once.

### Pulling state from the bean graph

The `_with` variants pass the resolved `BeanContext`, so you can look state up
by type instead of threading it through:

```rust
app.schedule_tasks_with(|ctx| {
    let svc = ctx.get::<SyncService>(); // panics if missing; ctx.try_get returns Option
    sources
        .iter()
        .map(|source| {
            let source = source.clone();
            ScheduledTaskDef::new(
                format!("sync_{}", source.name),
                source.schedule.clone(),
                svc.clone(),
                move |svc| {
                    let source = source.clone();
                    async move { svc.sync(&source).await }
                },
            )
        })
        .collect::<Vec<_>>()
})
```

`schedule_task_with` is the single-task version.

## Building a `ScheduledTaskDef`

```rust
// With state: the closure gets a clone of the state on every tick.
ScheduledTaskDef::new("sync_users", "5m".parse()?, user_service, |svc| async move {
    svc.sync().await
});

// Without state.
ScheduledTaskDef::from_fn("heartbeat", "30s".parse()?, || async {
    tracing::info!("still alive");
});
```

The closure returns `()` or `Result<(), E>` with `E: Display`. Errors are logged
under the task name, the same way as for a failing `#[scheduled]` method.

The builder options match the `#[scheduled]` attribute keys:

| Method | `#[scheduled]` equivalent | Effect |
|---|---|---|
| `.with_overlap(OverlapPolicy::Concurrent)` | `overlap = "concurrent"` | Let a tick start while the previous one still runs |
| `.with_skip_if(\|svc\| async move { .. })` | `skip_if = "method"` | Return `true` to skip this tick. The schedule keeps going, and the skip counts in `ScheduledJobInfo::skip_count`. |

`into_boxed_any()` gives the type-erased form, for code that manages the task
registry by hand (tests, unusual plugins).

## Schedules from configuration

`ScheduleConfig` parses from a string:

```rust
let a: ScheduleConfig = "30s".parse()?;            // interval
let b: ScheduleConfig = "1h30m".parse()?;          // compound duration
let c: ScheduleConfig = "0 */5 * * * *".parse()?;  // cron (contains whitespace)
let d: ScheduleConfig = "@hourly".parse()?;        // cron shortcut
```

| Input | Result |
|---|---|
| Duration string (`ms`, `s`, `m`, `h`, `d`, combinable) | Interval |
| String with whitespace, or starting with `@` | Cron expression, checked when parsed |
| Integer config value | Interval in seconds |

So a typed config section can hold a schedule directly:

```rust
#[config_section("app.sync")]
pub struct SyncConfig {
    schedule: ScheduleConfig, // app.sync.schedule: "5m", "0 0 2 * * *" or 300
}
```

`r2e::r2e_scheduler::parse_duration("1h15m30s")` parses the same duration
grammar and returns a `PositiveDuration` (or an error message).

## Limitations

Tasks cannot be added to an app that is already serving. That would need a
"start now" handle, which does not exist yet.

## Choosing an approach

| Need | Use |
|---|---|
| Tasks known at compile time | `#[scheduled(every = ..)]` / `#[scheduled(cron = ..)]` on a bean or controller |
| Runtime task sets on the shared scheduler | `schedule_task` / `schedule_tasks` (this page) |
| Custom registry handling | `ScheduledTaskDef::into_boxed_any()` |

## Testing

A `TestApp` boot never runs the scheduler driver, so no task ticks on its own
during a test. Enable `scheduler-testing` in `[dev-dependencies]` and fire a
task by name:

```rust
use r2e::r2e_scheduler::testing::{SchedulerTestExt, TickOutcome};

#[r2e::test]
async fn sync_runs() {
    let app = TestApp::boot::<MyApp>().await;
    assert_eq!(app.scheduler().run("sync_users").await, TickOutcome::Ran);
}
```

The tick is the one production runs: the `skip_if` check, then the body, on the
executor pool. `TickOutcome::Skipped` reports a skipped tick, and the
`ScheduledJobInfo` counters move as in production.

## See also

- Full guide: [docs/features/21-dynamic-scheduled-tasks.md](https://github.com/plawn/r2e/blob/master/docs/features/21-dynamic-scheduled-tasks.md)
- [Scheduling](./scheduling.md)
- [Executor and Background Services](./background-services.md)
