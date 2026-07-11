# Feature 21 — Dynamic (Config-Driven) Scheduled Tasks

## Objective

Make runtime-defined task sets — one task per configured source, tenant, or feed — registrable through a **public scheduler API**, instead of reaching into internals (`get_plugin_data::<TaskRegistryHandle>()` + `ScheduledTaskMarker` + hand-double-boxed `ScheduledTaskDef`s). `#[scheduled]` remains the right tool for statically-known tasks; this API covers the rest.

## `AppBuilderSchedulerExt`

`schedule_task` / `schedule_tasks` live on the post-`build_state()` builder (the same shape as `register_grpc_service`):

```rust
use r2e_scheduler::{AppBuilderSchedulerExt, ScheduledTaskDef, Scheduler};

let app = AppBuilder::new()
    .plugin(Scheduler)                 // required — before build_state()
    .provide(sync_service.clone())
    .build_state()
    .await;

// e.g. one sync task per configured source
let app = sources.iter().fold(app, |app, source| {
    let svc = sync_service.clone();
    let source = source.clone();
    app.schedule_task(ScheduledTaskDef::new(
        format!("sync_{}", source.name),
        source.schedule.clone(),       // ScheduleConfig
        svc,
        move |svc| {
            let source = source.clone();
            async move { svc.sync(&source).await }   // () or Result<(), E: Display>
        },
    ))
});

app.serve("0.0.0.0:3000").await;
```

Dynamic tasks share the static tasks' lifecycle: started by the scheduler's serve hook, listed in `ScheduledJobRegistry`, stopped via the shared `CancellationToken` on shutdown.

- **Requires `.plugin(Scheduler)`** before `build_state()` — `schedule_task` panics with a clear message otherwise.
- **Register before `serve()`** — the task registry is drained once at serve time; tasks added after boot are never started.
- `schedule_tasks(impl IntoIterator<Item = ScheduledTaskDef<T>>)` registers a batch with one registry lock.

## Building task definitions

```rust
// Stateful: the closure receives a clone of the state each tick.
ScheduledTaskDef::new("sync_users", "5m".parse()?, user_service, |svc| async move {
    svc.sync().await          // Result<(), E> — errors are logged under the task name
});

// Stateless: move captures into the closure.
ScheduledTaskDef::from_fn("heartbeat", "30s".parse()?, || async {
    tracing::info!("still alive");
});
```

Closures may return `()` or `Result<(), E: Display>` — the error branch is logged exactly like a failing `#[scheduled]` method (`ScheduledResult` contract).

For advanced flows that manage the registry by hand, `ScheduledTaskDef::into_boxed_any()` produces the type-erased shape `TaskRegistryHandle` stores (the counterpart of `extract_tasks`), so the double-box never appears in user code.

## Config-driven schedules

`ScheduleConfig` parses from strings and from config values:

```rust
let cfg: ScheduleConfig = "30s".parse()?;            // duration → Interval
let cfg: ScheduleConfig = "1h30m".parse()?;          // compound durations
let cfg: ScheduleConfig = "0 */5 * * * *".parse()?;  // whitespace → validated Cron
let cfg: ScheduleConfig = "@hourly".parse()?;        // @-shortcuts → Cron
```

- `FromStr`: duration strings (`ms`/`s`/`m`/`h`/`d`, combinable — same grammar as `#[scheduled(every = "...")]`) become `Interval`; anything with whitespace or a leading `@` is validated as a cron expression.
- `FromConfigValue`: a config string parses as above; an integer is seconds (mirroring `#[scheduled(every = 30)]`). So typed config sections can declare schedules directly:

```rust
#[config_section("app.sync")]
pub struct SyncConfig {
    schedule: ScheduleConfig,    // app.sync.schedule: "5m" or "0 0 2 * * *" or 300
}
```

- `parse_duration("1h15m30s") -> Result<Duration, String>` is public for callers that need raw durations (runtime twin of the compile-time parser in `r2e-macros`; proc-macro crates can't export functions).

## Prelude

`r2e_scheduler::prelude` (new) re-exports `AppBuilderSchedulerExt`, `ScheduleConfig`, `ScheduledTaskDef`, `Scheduler`, `SchedulerHandle`, `ScheduledJobRegistry`, `ScheduledJobInfo`; the `r2e` facade prelude includes it behind the `scheduler` feature — `use r2e::prelude::*` is enough.

## Not covered (by design)

Post-serve registration: serve hooks drain the task registry once at serve time. Registering tasks on a live, already-serving app would need a "start now" handle (serve-time token + job registry) that doesn't exist yet — out of scope until a real app needs it.

## Escape-hatch ladder

1. `#[scheduled(every/cron/...)]` — statically-known tasks on controllers/beans.
2. `AppBuilderSchedulerExt::schedule_task(s)` — runtime-defined task sets, still on the shared scheduler lifecycle. **This feature.**
3. `ScheduledTaskDef::into_boxed_any()` + `TaskRegistryHandle` — custom registry management (tests, exotic plugins).
4. `ScheduledTask` trait impl + `start(CancellationToken)` — fully custom task types and spawning.
