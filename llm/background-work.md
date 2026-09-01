---
topic: background-work
features: executor
tokens: ~2700
requires: di-beans
---

## Background Work (Executor)

### TL;DR

- Requires feature `executor`; `.plugin(Executor)` provides the `PoolExecutor` bean — `#[inject]` it and `executor.spawn(..)` returns a `JobHandle`.
- Prefer the pool over a raw `tokio::spawn`: pool jobs are tracked, bounded and drained at shutdown.
- `#[async_exec]` (on `#[bean]` impls and `#[routes]` controllers) submits the body to the `PoolExecutor` field — default field name `executor`, override with `#[async_exec(executor = "field")]` — and returns `Result<JobHandle<T>, RejectedError>` synchronously.
- `#[async_exec]` cannot be combined with `#[scheduled]`/`#[consumer]`/`#[post_construct]`/`#[pre_destroy]`/`#[on_start]`/`#[intercept]`, nor on a controller with a route verb, `#[fallback]`, `#[sse]` or `#[ws]`; an impl-level `#[intercept]` never wraps it.
- Long-running workers are `#[derive(BackgroundService)]`: app-scoped fields only (`#[inject]`, `#[config]`, `#[config_section]`, `#[live_config]`), and you write `async fn run(&self, shutdown: rt::CancelToken)`.
- Call `.spawn_service::<S>()` AFTER `build_state()` (it needs the graph); `try_spawn_service` returns `Result<_, ConfigValidationError>` instead of panicking.
- Every service MUST observe its `CancelToken`; the shutdown join is bounded per handle by `shutdown_grace_period`, and a service that ignores it is abandoned with a `warn!`.
- Gate one service with `#[service(enabled = "field_or_method")]` and all of them with `services.enabled: false` — both skip only `run()`, never registration, `from_context`, or config validation.
- For a worker type you do not own, use `#[producer(start)]` (bean + service in one function) instead of writing an adapter struct; a hand-written `impl ServiceComponent` must supply `type Deps` (`TNil` when it reads nothing).
- Startup work that must not repeat under `r2e dev` goes in `.on_start_once(..)` / `#[on_start(once)]`, not a hand-rolled `static BOOTED`.

Requires feature: `executor`. Managed task pool with bounded concurrency and
graceful drain:

```rust
# fn __doc(b: AppBuilder) -> impl Sized {
b.plugin(Executor)                         // provides PoolExecutor bean
# }

#[controller]
pub struct JobController {
    #[inject] executor: PoolExecutor,
}
// executor.spawn(...) → JobHandle; or #[async_exec] methods; or
// #[derive(BackgroundService)] for long-running service components
# fn main() {}
```

`#[async_exec]` works on `#[bean]` impls and `#[routes]` controllers alike:
the `async fn(&self, ...) -> T` body is submitted to the `PoolExecutor` held
in a field (default name `executor`; override with
`#[async_exec(executor = "field")]`) and the call returns
`Result<JobHandle<T>, RejectedError>` synchronously. Cannot be combined with
`#[scheduled]`/`#[consumer]`/`#[post_construct]`/`#[pre_destroy]`/`#[on_start]`/`#[intercept]`
on one method —
and on a **controller** also not with a route verb (`#[get]`, `#[post]`, …),
`#[fallback]`, `#[sse]`, or `#[ws]` (the pool-submission rewrite and a route
registration are mutually exclusive). Note: an **impl-level** `#[intercept]`
wraps only the `#[scheduled]`/`#[consumer]` methods in the block — it does NOT
wrap `#[async_exec]` methods (their wrapper runs no interceptor chain); only a
method-level `#[intercept]` on an `#[async_exec]` method is an error.

```rust
#[derive(Clone)]
pub struct ReportService { executor: PoolExecutor }

#[bean]
impl ReportService {
    pub fn new(executor: PoolExecutor) -> Self { Self { executor } }

    #[async_exec]
    async fn generate_pdf(&self, id: u64) -> Vec<u8> { render_pdf(id).await }
}
// let job = report_service.generate_pdf(7)?;  let bytes = job.await?;
```

The `Scheduler` plugin runs every `#[scheduled]` tick on this pool, so scheduled
work is drained, bounded, and metered alongside submitted jobs.

Prefer this over raw `tokio::spawn` — jobs are tracked and drained on shutdown.

### Long-running services — `#[derive(BackgroundService)]`

A service component is built from the bean graph and started once, with an
`rt::CancelToken` cancelled at shutdown. Fields use the app-scoped injection
attributes (`#[inject]`, `#[config]`, `#[config_section]`, `#[live_config]`);
you write `run`, the derive writes `ServiceComponent`:

```rust
#[derive(BackgroundService)]
pub struct MetricsExporter {
    #[inject] pool: SqlitePool,
    #[config("metrics.interval-secs")] interval: u64,
}

impl MetricsExporter {
    async fn run(&self, shutdown: rt::CancelToken) { /* loop until cancelled */ }
}

# async fn __doc() -> impl Sized {
AppBuilder::new()
    .register::<CreatePool>()
    .load_config::<RootConfig>()
    .build_state().await
    .spawn_service::<MetricsExporter>()    // AFTER build_state (needs the graph)
    .serve("0.0.0.0:3000").await
# }
# fn main() {}
```

`spawn_service` comes from the `SpawnService` extension trait (in the prelude),
like `register_controller`. Each service gets its own `rt::CancelToken`,
minted as a **child of the app shutdown token** (itself created lazily on the
first `register_service`/`run()` and shared through `plugin_data`): the normal
shutdown sequence
cancels it early (before the HTTP drain) through a plugin shutdown hook, and the
exits that run no hook — a panic, or `r2e dev` dropping the `run()` future —
still reach it when the app token's drop guard fires. Services must observe
their token; the join at shutdown is bounded per handle by
`shutdown_grace_period` (a service that ignores its token is abandoned with a
`warn!` naming its type, and `on_stop` still runs). The derive declares `ServiceComponent::Deps` (every
`#[inject]` type, plus `R2eConfig` / `LiveConfigRegistry` when config fields are
present), so a service reading an absent bean is a **compile error** at
`spawn_service` instead of a panic at startup; and it declares
`ServiceComponent::config_keys()` + `ServiceComponent::config_sections()`, so a
missing required `#[config]` key **and** an incomplete `#[config_section]` are
reported by aggregated startup validation. `try_spawn_service` returns
`Result<_, ConfigValidationError>` instead of panicking.

Hand-written `impl ServiceComponent` must supply `type Deps` (use
`r2e_core::type_list::TNil` when the service reads nothing from the graph) and
may override `config_keys()` / `config_sections()`.

#### Turning a service off — `#[service(enabled = "…")]`

```rust
#[derive(BackgroundService)]
#[service(enabled = "enabled")]
pub struct MetricsExporter {
    #[config("metrics.enabled")] enabled: bool,
    #[inject] pool: SqlitePool,
}

impl MetricsExporter {
    async fn run(&self, shutdown: rt::CancelToken) { /* loop until cancelled */ }
}
```

The name is looked up among the struct's own fields first (the usual case — a
config flag), and read as a `&self` method returning `bool` otherwise. **Only
`run()` is skipped**: the service is still registered, its beans still resolve,
`from_context` still runs, and its `#[config]` / `#[config_section]` keys are
still presence-validated — turning a worker off never turns its configuration
errors off with it. One `info!` names the service and the gate (the *config
key*, when the derive can see one). The gate is read at spawn time, on the
constructed instance, on **every** spawn path (`spawn_service` and
`#[producer(start)]`). Hand-written impls override `fn enabled(&self) -> bool`
(default `true`) and `fn enabled_gate() -> Option<&'static str>`.

#### Turning **all** services off — `services.enabled: false`

```yaml
# application-test.yaml — a test boot assembles the app, it does not run workers
services:
  enabled: false
```

The profile-level switch: no background service calls `run()` on any spawn path.
It composes with the per-service gate (a service runs only when **both** say
yes), defaults to `true` when the key is absent, and — like the per-service gate
— skips only `run()`: registration, `from_context` and config validation stay
unconditional, so a test with services off still fails on a broken service
config. One `info!` for the whole process, not one per service. There is no
sniffing of the profile: it is an ordinary config key
(`r2e_core::runtime::service::SERVICES_ENABLED_KEY`), so it can be flipped from
`application-<profile>.yaml`, `R2E_SERVICES_ENABLED`, or
`override_config_value("services.enabled", false)` in a test.

#### You do NOT need an adapter struct — `#[producer(start)]`

When the worker is a plain type from a shared crate — ordinary fields, an
ordinary constructor — do **not** write a second struct whose only job is to
hold `#[inject]` fields and clone them across. `#[producer(start)]` registers
its output as a bean *and* runs it as a service, so the app builds the worker
from beans and config in one function:

```rust
// shared crate: no DI, no attributes
#[derive(Clone)]
pub struct Reindexer { sink: Sink, batch_size: usize }

impl Reindexer {
    pub fn new(sink: Sink, batch_size: usize) -> Self { Self { sink, batch_size } }
    pub async fn run(&self, shutdown: rt::CancelToken) { /* loop */ }
}

// The produced value IS a bean, so the service reads itself back out of the
// graph. Three lines, and they can live in the shared crate too.
impl ServiceComponent for Reindexer {
    type Deps = TCons<Reindexer, TNil>;
    fn from_context(ctx: &BeanContext) -> Self { ctx.get::<Reindexer>() }
    async fn start(self, shutdown: rt::CancelToken) { self.run(shutdown).await }
}

// app crate: one function, no adapter struct (the producer generates a
// `BuildReindexer` marker — name it so it cannot collide with `Reindexer`)
#[producer(start)]
fn build_reindexer(sink: Sink, config: R2eConfig) -> Reindexer {
    Reindexer::new(sink, config.get_or("search.batch", 500))
}
```

Use the derive when the worker's fields *are* the injected beans; use
`#[producer(start)]` when the worker is someone else's type with a plain
constructor. `after(...)` composes with it when the worker must start behind
another bean.

`#[producer]` folds `<Output as ServiceComponent>::Deps` into the producer's
own registration deps, so a service reading a bean the producer itself never
takes is still a **compile error** at `build_state()`; the service's declared
keys and sections are validated during `build_state()`, merged with the bean
graph's own into one `BeanError::MissingConfigKeys`.

#### Startup work that must not repeat under `r2e dev`

A hot patch re-assembles the app in the same process. Work a boot may do only
once — crash recovery, claiming a lock, a one-off backfill — goes in
`.on_start_once(...)` or `#[on_start(once)]` instead of a hand-rolled
`static BOOTED: AtomicBool`. In production it is identical to `on_start` (one
boot = one cycle); under `r2e dev` it runs on the first cycle only. A patch
that *changes* the closure does not re-run it. Note that `spawn_service`
services are cancelled by a hot patch and **not** restarted for the rest of the
dev session — restart the process when iterating on a service body.
