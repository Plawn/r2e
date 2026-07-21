# r2e-scheduler

Background task scheduler for R2E — interval, cron, and delayed task execution.

## Overview

Provides a scheduler plugin that auto-discovers `#[scheduled]` methods on controllers and beans and runs them from a single driver task backed by a min-heap of next-fire times (not one Tokio task per schedule). Each tick body is submitted to the shared `PoolExecutor` (from the `Executor` plugin); a job is re-armed only when its tick completes, so a task never overlaps with itself while different jobs still run concurrently. Tasks are lifecycle-managed via `CancellationToken` for clean shutdown, and in-flight ticks drain through the pool.

**Requires the `Executor` plugin.** `Scheduler` declares `type Deps = (PoolExecutor,)`, so `.plugin(Scheduler)` without a `PoolExecutor` in the graph fails at `build_state()` with a guided "missing `.provide::<PoolExecutor>()` / `.register::<PoolExecutor>()`" error. The `scheduler` facade feature pulls in `executor`.

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.1", features = ["scheduler"] }   # pulls in "executor"
```

## Setup

Install the `Executor` and `Scheduler` plugins **before** `build_state()` (order between them does not matter):

```rust
use r2e::r2e_scheduler::Scheduler;
use r2e::r2e_executor::Executor;

AppBuilder::new()
    .plugin(Executor)                       // required by Scheduler (ticks run on the pool)
    .plugin(Scheduler)
    .register::<CleanupService>()
    .build_state()
    .await
    .register_controller::<ScheduledJobs>()
    .serve("0.0.0.0:3000")
    .await;
```

## Declarative scheduling

```rust
#[controller(path = "/jobs")]
pub struct ScheduledJobs {
    #[inject] service: CleanupService,
}

#[routes]
impl ScheduledJobs {
    #[scheduled(every = 30)]                     // every 30 seconds
    async fn cleanup(&self) {
        self.service.cleanup_expired().await;
    }

    #[scheduled(every = 60, initial_delay = 10)] // first run after 10s, then every 60s
    async fn sync(&self) {
        self.service.sync_external().await;
    }

    #[scheduled(cron = "0 */5 * * * *")]         // cron: every 5 minutes
    async fn report(&self) {
        self.service.generate_report().await;
    }

    #[scheduled(every = 300, skip_if = "in_maintenance")] // skip while a predicate holds
    async fn purge(&self) {
        self.service.purge().await;
    }

    // `skip_if` names a `&self`-only method returning `bool` on the same impl.
    fn in_maintenance(&self) -> bool {
        self.service.maintenance_mode()
    }
}
```

## Schedule types

| Config | Syntax | Description |
|--------|--------|-------------|
| Interval | `every = 30` | Fixed interval in seconds |
| Interval + delay | `every = 60, initial_delay = 10` | Initial delay before first run |
| Cron | `cron = "0 */5 * * * *"` | Standard cron expression |
| Overlap | `overlap = "skip" \| "concurrent"` | Self-overlap policy (default `skip`) |
| Skip predicate | `skip_if = "method"` | Skip a tick when a `&self`-only method returning `bool` yields `true` (Quarkus `skipExecutionIf`) |

Durations accept a bare integer (seconds — `every = 30`) or a duration string
(`every = "5m"`, `initial_delay = "10s"`). `initial_delay` only pairs with
`every`, not `cron`.

## Overlap policy

`#[scheduled(overlap = "concurrent")]` (also valid with `cron`) lets a job overlap
with itself: the next tick is armed at fire time so a slow tick never holds back
the following one. The default `skip` re-arms on completion, so a job never runs
concurrently with itself (a due-while-running tick is skipped, cadence preserved).
Dynamic tasks: `ScheduledTaskDef::new(..).with_overlap(OverlapPolicy::Concurrent)`.

## Skip predicate

`#[scheduled(skip_if = "method")]` (Quarkus `skipExecutionIf`) names a predicate
on the same impl block — `fn method(&self) -> bool`, sync or async, `&self` only.
It runs at the start of every tick (scheduled and `trigger_now` alike); returning
`true` suppresses that tick's body. The schedule keeps advancing and the skip is
counted in `ScheduledJobInfo::skip_count` (not `run_count`). Dynamic tasks:
`ScheduledTaskDef::new(..).with_skip_if(|state| async move { .. })`.

## Configuration (`scheduler.*`)

Optional typed config. `scheduler.enabled = false` skips starting tasks (beans
remain). `scheduler.executor` selects the pool ticks run on: `"shared"` (default,
the app-wide `PoolExecutor`) or `"dedicated"` (a private pool sized by
`scheduler.max-concurrent` / `queue-capacity` / `shutdown-timeout`, mirroring
`executor.*`, with its own graceful drain). `PoolExecutor` stays a required
`Deps` even in dedicated mode; an unknown `executor` value panics at boot.

## Runtime control and stats

Extract a `SchedulerHandle` (or `SchedulerHandle::channel(token)` for a manual
`start_jobs`) to control jobs by name: `pause(name).await`, `resume(name).await`,
`trigger_now(name).await` (each `-> bool`). A paused job advances its cadence but
never fires; `trigger_now` fires once out of band (even when paused). Live per-job
stats live on `ScheduledJobInfo` via `ScheduledJobRegistry::list_jobs()` /
`job(name)`: `last_run`, `next_run`, `last_duration`, `run_count`, `skip_count`,
`panic_count`, `paused`.

## Lifecycle

1. `Scheduler` plugin creates a `CancellationToken` during `plugin()` phase
2. `build_state()` verifies the `PoolExecutor` dependency (`Scheduler::Deps`)
3. Tasks are collected from controllers during `register_controller()`
4. `serve()` spawns ONE driver task (`start_jobs`) that owns a min-heap of next-fire deadlines for all schedules; when the earliest deadline is reached, due tick bodies are submitted to the shared `PoolExecutor` and each job is re-armed only when its own tick completes
5. On shutdown signal (Ctrl-C / SIGTERM), the `CancellationToken` is cancelled; the driver stops without aborting in-flight ticks, which drain via the pool (`executor.shutdown-timeout`)

Because ticks run as pool jobs, a panicking tick is contained and logged (the driver keeps running), scheduled work is bounded by `executor.max-concurrent`, and it shows up in `ExecutorMetrics`.

## Constraints

Scheduled tasks run on the controller core, which always implements `ContextConstruct` (it builds from the resolved `BeanContext` by type; identity and request-scoped fields live only on the per-request façade). Controllers can be used for scheduling regardless of any struct-level or param-level `#[inject(identity)]`.

## License

Apache-2.0
