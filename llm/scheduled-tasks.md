---
topic: scheduled-tasks
features: scheduler
tokens: ~3200
requires: di-beans, background-work
---

## Scheduled Tasks

### TL;DR

- Requires feature `scheduler`; install `.plugin(Executor)` **and** `.plugin(Scheduler)` before `build_state()` (order between them is free) — Scheduler declares `type Deps = (PoolExecutor,)` and fails at `build_state()` without it. For the pool itself see llm/background-work.md.
- `#[scheduled(every = 30)]` is seconds; `every = "5m"` / `initial_delay = "10s"` are duration strings; `cron = "0 */5 * * * *"` is validated at compile time.
- `#[scheduled]` works the same on `#[routes]` controllers and `#[bean]` impls, and is auto-collected — `register_controller::<C>()` / `.register::<T>()` is the whole registration.
- Default overlap is `skip` (re-arm on completion, so a job never overlaps itself and `next_run` reads `None` while its tick runs); `overlap = "concurrent"` re-arms at fire time.
- `skip_if = "method"` names a plain `&self -> bool` method (sync or async) in the SAME impl block — pointing anywhere else, or at a marked method, is a compile error; skipped ticks count in `skip_count`, not `run_count`.
- `#[scheduled(every = "0s")]` is a compile error (intervals carry a `PositiveDuration`); a schedule may come from config via `ScheduleConfig` (`FromStr`/`FromConfigValue`).
- `#[bean(lazy)]` + `#[scheduled]`, and `#[scheduled]` + `#[consumer]` on one method, are compile errors.
- To `#[intercept]` bean `#[scheduled]`/`#[consumer]` methods, `#[bean]` must ALSO annotate the struct (it injects the hidden decorator slot); a sync intercepted method is promoted to `async fn`.
- `.override_bean(instance)` skips the bean's registration, so it runs **undecorated** and its scheduled tasks are dropped — pin its dependencies instead, or opt in with `.override_bean_decorated(..)` / `svc.decorate(app.bean_context())`.
- Introspect with `#[inject] jobs: ScheduledJobRegistry`; drive at runtime with `SchedulerHandle::{pause, resume, trigger_now}` — each answers `bool`, and `false` means the command was refused.

Requires feature: `scheduler` (pulls in `executor`). **The Scheduler requires the
Executor plugin** — it declares `type Deps = (PoolExecutor,)`, so `.plugin(Scheduler)`
without a `PoolExecutor` in the graph fails at `build_state()` with the guided
"missing `.provide::<PoolExecutor>()` / `.register::<PoolExecutor>()`" error. Install
both **before** `build_state()` (order between them does not matter):

```rust
# async fn __doc(b: AppBuilder) -> impl Sized {
b.plugin(Executor)                          // provides PoolExecutor (Scheduler runs ticks on it)
 .plugin(Scheduler)
 .build_state().await
 .register_controller::<ScheduledJobs>()   // auto-discovers #[scheduled] methods
# }
```

All schedules are driven by a single driver task backed by a min-heap of next-fire
times (not one Tokio task per schedule). Each tick runs as a pool job
(`executor.submit(...)`); when a job is re-armed is its overlap policy: `Skip` (default)
re-arms only when its own tick completes, so a task never overlaps with itself
(non-overlap preserved, and `next_run` reads `None` while its tick runs — it is `Some`
only while a live driver holds a deadline, so it also reads `None` for every job once
the driver stops), while
`Concurrent` re-arms at fire time, before the tick is built. Different jobs always run
concurrently. Consequences: in-flight ticks drain on shutdown (bounded by
`executor.shutdown-timeout`), a panicking tick is contained/logged and the driver keeps
running, and scheduled work is globally bounded by `executor.max-concurrent` and shows up
in `ExecutorMetrics`.

```rust
#[controller]
pub struct ScheduledJobs {
    #[inject] user_service: UserService,
}

#[routes]
impl ScheduledJobs {
    #[scheduled(every = 30)]                          // every 30s (integer = seconds)
    async fn count_users(&self) {
        tracing::info!(users = self.user_service.count().await);
    }

    #[scheduled(every = "5m", initial_delay = "10s")] // duration strings
    async fn sync(&self) { self.user_service.sync().await; }

    #[scheduled(cron = "0 */5 * * * *")]              // compile-time validated
    async fn report(&self) { tracing::info!("report"); }

    #[scheduled(every = "50ms", overlap = "concurrent")] // may overlap with itself
    async fn poll(&self) { tracing::debug!("poll"); }

    // Skip predicate (Quarkus skipExecutionIf): a plain `&self -> bool` method
    // (sync or async) in the SAME impl block, checked before every tick.
    fn maintenance_mode(&self) -> bool { false }

    #[scheduled(every = "5m", skip_if = "maintenance_mode")]
    async fn refresh(&self) { self.user_service.sync().await; }
}
# fn main() {}
```

Overlap policy — `#[scheduled(overlap = "skip" | "concurrent")]` (default `skip`;
also valid with `cron`). `skip`: never run a job concurrently with itself (re-arm
on completion; a due-while-running tick is skipped, cadence preserved).
`concurrent`: re-arm at fire time so a slow tick never holds back the next one
(ticks may pile up). Dynamic tasks: `ScheduledTaskDef::new(..).with_overlap(OverlapPolicy::Concurrent)`.

Skip predicate — `#[scheduled(skip_if = "method")]`. `method` names a plain
`&self` method returning `bool` (sync or async), defined in the same impl block
(pointing anywhere else, or at a marked method, is a compile error). `true`
skips that tick's body — evaluated on every fire, scheduled and `trigger_now`
alike; the schedule keeps advancing. Skips are counted in
`ScheduledJobInfo::skip_count`; `run_count`/`last_run`/`last_duration` only
reflect ticks that actually ran. For a shared/injected condition, `#[inject]`
the predicate bean and delegate. Dynamic tasks:
`ScheduledTaskDef::new(..).with_skip_if(|state| async move { ... })`.

`#[scheduled]` also works on **beans** — same attribute, same options, and
registration is automatic (`.register::<T>()` alone; no extra call — same
auto-collection as `#[consumer]` beans):

```rust
# async fn __doc(b: AppBuilder, pool: SqlitePool) -> impl Sized {
#[derive(Clone)]
pub struct CleanupService { pool: SqlitePool }

#[bean]
impl CleanupService {
    pub fn new(pool: SqlitePool) -> Self { Self { pool } }

    #[scheduled(every = "5m")]
    async fn purge_stale(&self) {         // sync and `-> Result<(), E>` also OK
        sqlx::query("delete from sessions where expires_at < datetime('now')")
            .execute(&self.pool).await.ok();
    }
}

b.plugin(Executor).plugin(Scheduler)
 .provide(pool)                           // the bean CleanupService injects
 .register::<CleanupService>()            // #[scheduled] auto-collected
 .build_state().await
# }
```

Default task name is `<Type>_<method>` (`name = "..."` to override). Needs the
`scheduler` feature (the generated impl references `r2e_scheduler` types — same
requirement as controller `#[scheduled]`). Limits: `#[bean(lazy)]` +
`#[scheduled]` is a compile error, as is combining `#[scheduled]` with
`#[consumer]` on one method.

**Bean-level interceptors.** `#[scheduled]` and `#[consumer]` methods in a
`#[bean]` impl accept `#[intercept(...)]` (and an impl-level `#[intercept(...)]`
that wraps every such method, running before method-level ones). Same
`DecoratorSpec` values as on controllers; their `Deps` are folded into the
bean's registration deps (missing bean = compile error at `.register`). This
needs per-instance storage so DIRECT calls self-intercept too, so **`#[bean]`
must also annotate the struct** (it injects a hidden decorator slot):

```rust
#[bean]                        // struct form — injects the hidden slot
#[derive(Clone)]
pub struct CleanupService { ticks: Arc<AtomicUsize> }

#[bean]                        // impl form
#[intercept(Logged::info())]   // impl-level: wraps every scheduled/consumer method
impl CleanupService {
    pub fn new(ticks: Arc<AtomicUsize>) -> Self { Self { ticks } }

    #[scheduled(every = "5m")]
    #[intercept(AuditTick::spec("purge"))]   // method-level (after impl-level)
    async fn purge(&self) { self.ticks.fetch_add(1, Ordering::Relaxed); }
}
```

A sync `#[scheduled]` method with `#[intercept]` is promoted to `async fn`
(call it with `.await`). Interceptors on bean `#[consumer]` methods work for
both fan-out subscribers and request-reply responders. `#[intercept]` on a
plain (non-scheduled/consumer) bean method is a compile error. Struct literals
of the bean type *outside* the impl block need an explicit
`__r2e_decos: Default::default()` (the field is `pub #[doc(hidden)]`).

A plain `.override_bean(instance)` pin skips ALL of the bean's hooks, so the
pinned instance runs **undecorated**. Two explicit opt-ins re-enable decoration
for instances that bypass normal registration (default unchanged):

```rust
# async fn __doc(b: AppBuilder, app: AppBuilder<()>) -> impl Sized {
use r2e::Decorate;   // the trait behind `decorate()` — NOT in the prelude

// Hand-built instance — fill its slot from a resolved graph.
let svc = CleanupService::new(stub);
svc.decorate(app.bean_context());   // idempotent; clones after this share it
svc.purge().await;                   // now intercepted

// Pinned test double — pin AND decorate (scheduled tasks / post_construct still skipped)
b.override_bean_decorated(CleanupService::new(stub))
# }
```
In tests, pinning the intercepted bean itself (`override_bean(instance)`)
skips its registration → the slot is never filled → it runs **undecorated**
(and its scheduled tasks are dropped). To keep interceptors active, pin the
*dependencies* instead and let the graph build the intercepted bean.

Dynamic (config-driven) tasks — post-`build_state()`:

```rust
# fn __doc<S: Clone + Send + Sync + 'static>(app: AppBuilder<S>, schedule: ScheduleConfig) -> impl Sized {
use r2e::r2e_scheduler::{AppBuilderSchedulerExt, ScheduledTaskDef};   // both also in the prelude

app.schedule_task(ScheduledTaskDef::from_fn("heartbeat", "30s".parse().unwrap(),
    || async { tracing::debug!("tick") }))
.schedule_task_with(|ctx| ScheduledTaskDef::new(
    "sync", schedule, ctx.get::<SyncService>(),
    move |svc| async move { svc.sync().await }))
# }
```

Config (`scheduler.*` YAML section, all optional): `scheduler.enabled = false`
(standard gate — skips starting tasks, keeps beans); `scheduler.executor =
"shared"` (default, app-wide `PoolExecutor`) | `"dedicated"` (private pool sized
by `scheduler.max-concurrent` / `queue-capacity` / `shutdown-timeout`, mirroring
`executor.*`). `PoolExecutor` stays a hard `Deps` requirement even in
dedicated mode. An unknown `executor` value panics at boot.

Introspection: `#[inject] jobs: ScheduledJobRegistry` → `jobs.list_jobs()` /
`jobs.job(name)`. `ScheduledJobInfo` carries live stats: `last_run` /
`next_run` (`chrono::DateTime<Utc>`), `last_duration`, `run_count`,
`skip_count` (ticks suppressed by a `skip_if` predicate), `panic_count` (tick
bodies that panicked, plus tick-factory panics — those also auto-pause the job,
since construction fails identically on every fire; `resume` re-enables it),
`paused`.

Runtime control — `SchedulerHandle` (extract as a handler param, or
`SchedulerHandle::channel(token)` when driving `start_jobs` manually):
`pause(name).await` / `resume(name).await` (a paused job advances cadence but
never fires; `resume` replies `true` iff the job can fire again as far as the driver
can tell then — it keeps an already-armed deadline, reports the schedule's next
occurrence when a tick in flight will re-arm it, else re-arms from now (interval:
now+period, cron: next slot); a schedule that can never fire again stays unarmed and
`resume` replies `false`) / `trigger_now(name).await` (fire once out of band, allowed even
when paused; `false` for a `skip` job already in flight). Each returns `bool`, and
`false` means one of: unknown name; shutdown started / no driver (every
command refuses once the driver is cancelled — including one already queued, so a
command issued from a tick body during shutdown is a no-op, never a deadlock); for
`trigger_now`, a `skip` job already in flight, a closed executor pool (answered
first, then the driver stops), or a tick factory that panicked (contained; the job
is disabled); for `resume`, a spent/overflowed schedule. `pause` adds no case of
its own.
`ScheduleConfig` implements `FromStr`/`FromConfigValue` — a schedule can come
from config: `#[config("app.sync.schedule")] schedule: ScheduleConfig`.
Interval variants carry a `PositiveDuration` (a `Duration` guaranteed non-zero;
`PositiveDuration::from_secs(60).unwrap()` / derefs to `Duration`), so a zero
interval is unrepresentable; `parse_duration("5m")` returns `PositiveDuration`.
`#[scheduled(every = "0s")]` is a compile error.
