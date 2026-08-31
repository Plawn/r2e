# R2E Executor — Managed Task Pool & Background Services

The `r2e-executor` crate provides a managed task pool (`PoolExecutor`) and
ergonomic primitives for off-request work — analogous to JEE's
`ManagedExecutorService` and Quarkus's `@ApplicationScoped @Startup` services.

Three pieces:

1. `PoolExecutor` — bounded, semaphore-gated Tokio task pool. Injectable bean.
2. `#[async_exec]` — method attribute on a `#[bean]` impl or a `#[routes]`
   controller that submits the body to the pool and returns a
   `Result<JobHandle<T>, RejectedError>` instead of `T`.
3. `#[derive(BackgroundService)]` — DI-friendly `ServiceComponent<S>` for
   long-running workers (consumers, watchers, periodic jobs).

## Crate setup

```toml
# Cargo.toml
r2e = { workspace = true, features = ["executor"] }
```

`r2e::r2e_executor::*` (or `use r2e::prelude::*` for the macros) gives you:

- `Executor` — the plugin
- `PoolExecutor`, `RejectedError`,
  `ExecutorMetrics`, `ExecutorConfig`
- `BackgroundService` derive, `#[async_exec]` attribute

## PoolExecutor

```rust
use r2e::r2e_executor::{Executor, PoolExecutor, RejectedError};

AppBuilder::new()
    .plugin(Executor)              // installs PoolExecutor as a bean
    .load_config::<()>()           // loads R2eConfig (the plugin reads executor.*)
    // ...
    .build_state().await
```

The plugin reads the `executor.*` section of `R2eConfig`:

```yaml
executor:
  max-concurrent: 32     # tokio Semaphore permits — running cap
  queue-capacity: 1024   # pending submissions before rejection
  shutdown-timeout: 30s  # or: 30, "1m", "500ms"
```

`shutdown-timeout: 0` means "abort on shutdown, do not drain".

### API

```rust
let exec: PoolExecutor = state.executor.clone();

// Returns Result — Err(Shutdown) if pool is closed.
let h = exec.submit(async { 21 + 21 }).expect("pool running");
let v: u32 = h.await.expect("task ok");

// Bounded: errors with RejectedError::QueueFull when (running + queued) > cap.
match exec.try_submit(async { /* ... */ }) {
    Ok(h)                             => { /* h: JobHandle<T> */ },
    Err(RejectedError::QueueFull)     => { /* backpressure */ },
    Err(RejectedError::Shutdown)      => { /* pool closed */ },
}

// Fire-and-forget — useful inside background loops.
exec.submit_detached(async move { /* ... */ });

// Live snapshot — exposed for /metrics-style probes.
let m = exec.metrics(); // running / queued / completed / rejected (u64)
```

### Shutdown

The plugin registers an async `on_shutdown` hook that calls
`PoolExecutor::shutdown_graceful(timeout)` to drain in-flight tasks. After shutdown:

- `submit` / `try_submit` return `Err(RejectedError::Shutdown)`.
- Queued tasks that never acquired a permit are cancelled (the `JobHandle` resolves to a `JoinError` with `is_panic() == true`).
- Tasks already running finish naturally — bounded by `shutdown-timeout`.

## `#[async_exec]`

Marks a method on a `#[bean]` impl or a `#[routes]` controller as a
pool-executed job (W10: transverse member attributes are bean-level; a
controller may carry it because a controller core IS a bean). The generated
wrapper:

- Takes the same arguments as the original method.
- Returns `Result<JobHandle<T>, RejectedError>` instead of `T`.
- Is **not** `async` — the synchronous handle resolves to the result.

On a bean — the idiomatic home for heavy service work:

```rust
#[derive(Clone)]
pub struct ReportService {
    executor: PoolExecutor,
}

#[bean]
impl ReportService {
    pub fn new(executor: PoolExecutor) -> Self {
        Self { executor }
    }

    #[async_exec]                                     // default executor field: `executor`
    async fn generate_pdf(&self, id: u64) -> Vec<u8> {
        // ...heavy work...
        format!("PDF #{id}").into_bytes()
    }
}
```

On a controller — same attribute, same codegen:

```rust
#[controller(path = "/")]
#[derive(Clone)]
pub struct ReportController {
    #[inject] executor: PoolExecutor,
}

#[routes]
impl ReportController {
    #[post("/reports/:id")]
    async fn create(&self, Path(id): Path<u64>) -> Json<()> {
        // Returns immediately; PDF builds on the pool.
        let _job = self.generate_pdf(id).expect("executor running");
        Json(())
    }

    #[get("/reports/:id")]
    async fn fetch(&self, Path(id): Path<u64>) -> Json<usize> {
        // Awaits the result inline — useful when the caller wants the bytes.
        let bytes = self.generate_pdf(id).expect("executor running").await.expect("task ok");
        Json(bytes.len())
    }

    #[async_exec]
    async fn generate_pdf(&self, id: u64) -> Vec<u8> {
        format!("PDF #{id}").into_bytes()
    }
}
```

Override the executor field with `#[async_exec(executor = "io_pool")]`.

**Constraints (compile-time):**

- The annotated method must be `async fn(&self, ...) -> T`.
- The bean/controller must be `Clone + Send + Sync + 'static`
  (beans already are; `#[controller]` already implies this).
- The named field must implement
  `r2e_executor::PoolExecutor`-compatible `submit(...)` — typically a
  `PoolExecutor` bean (constructor param on a bean, `#[inject]` field on a
  controller).
- `#[async_exec]` cannot be combined with `#[scheduled]`, `#[consumer]`,
  `#[post_construct]`, or `#[intercept]` on the same method. On a **controller**
  it additionally cannot be combined with a route verb (`#[get]`, `#[post]`, …),
  `#[fallback]`, `#[sse]`, or `#[ws]` — the pool-submission rewrite and a route
  registration are mutually exclusive, so the combination is a compile error
  (it would otherwise silently 404 or drop the rewrite). The whole matrix is
  enforced by one shared validator, `validate_async_exec_method` in
  `r2e-macros/src/extract/async_exec.rs`, called by both hosts.
- **Impl-level `#[intercept(...)]` does not wrap `#[async_exec]` methods.** An
  impl-level interceptor on a `#[bean]` / `#[routes]` block applies only to the
  `#[scheduled]`/`#[consumer]` methods (which have a dispatch wrapper); it
  silently skips any `#[async_exec]` method in the same block, because the
  pool-submission wrapper runs no interceptor chain. This is allowed (so a mixed
  impl with consumers + an async_exec helper compiles); only a *method-level*
  `#[intercept]` on an `#[async_exec]` method is a hard error.

**Codegen** (shared emitter, `r2e-macros/src/codegen/transverse.rs`): the
original body is renamed `__r2e_async_<method>_inner` and a synchronous
wrapper takes its place, cloning `self`, capturing the executor, and
submitting an `async move` block. No registration hook — pure per-method
codegen, so it composes with `#[bean(lazy)]`.

## `#[derive(BackgroundService)]`

Generates `impl ServiceComponent` (no state generic) from the same `#[inject]` /
`#[config]` field syntax used by `#[controller]`. The component is built from
the resolved bean graph via `from_context(&BeanContext)` — each `#[inject]`
field resolved by type. The user supplies an `async fn run(&self,
rt::CancelToken)` method; the derived `start` just forwards to it. The emitted
`start` signature names the token through the resolved crate root
(`#krate::rt::CancelToken`), so a user crate needs neither `tokio-util` nor
`r2e-rt` in its manifest.

```rust
use r2e::rt::{self, CancelToken};

#[derive(BackgroundService, Clone)]
pub struct EmailWorker {
    #[inject] executor: PoolExecutor,
    #[inject] mailer: Mailer,
    #[config("email.batch_size")] batch_size: u64,
}

impl EmailWorker {
    async fn run(&self, shutdown: CancelToken) {
        let mut interval = rt::interval(Duration::from_secs(60));
        loop {
            rt::select! {
                _ = shutdown.cancelled() => break,
                _ = interval.tick() => {
                    let mailer = self.mailer.clone();
                    self.executor.submit_detached(async move {
                        let _ = mailer.flush().await;
                    });
                }
            }
        }
    }
}

// Register — uses the existing AppBuilder::spawn_service pipeline.
AppBuilder::new()
    .plugin(Executor)
    .build_state().await
    .spawn_service::<EmailWorker>()
    .serve_auto().await?;
```

`spawn_service::<C>()` collects the task handle so graceful shutdown
awaits the worker. The cancellation token is cancelled on shutdown
signal; the worker is expected to observe `shutdown.cancelled()` and
exit promptly.

There is no `#[service(state = ...)]`. The service resolves its `#[inject]`
fields from the bean graph by type (like a controller core), so it works with
the inferred HList state; each injected type must be present in the graph or
`spawn_service::<C>()` is a compile error naming the missing type.

### `#[service(enabled = "…")]` — the opt-in off switch

The one struct attribute the derive takes. It emits
`ServiceComponent::enabled()`; the name it takes is looked up among the
struct's own fields first (the usual case — a config flag), and read as a
`&self` method returning `bool` otherwise:

```rust
#[derive(BackgroundService)]
#[service(enabled = "enabled")]
pub struct EmailWorker {
    #[config("email.worker.enabled")] enabled: bool,
    #[inject] mailer: Mailer,
}
```

**Only `run()` is skipped.** The service is still registered, its beans are
still resolved, `from_context` still runs, and its `#[config]` /
`#[config_section]` keys are still presence-validated — turning a worker off
must never turn its configuration errors off with it (the same rule the
config subsystem follows: explicit config, no silent skips). The framework
logs one `info!` naming the service and the gate; when the gate is a
`#[config]`/`#[live_config]` field the log names the **config key**, which is
what an operator can act on, not the field.

The gate is read **at spawn time, on the constructed instance**, on every
spawn path — `spawn_service` / `SpawnService`, and the `#[producer(start)]` /
bean-declared service source. `per_worker_service` is closure-based rather
than a `ServiceComponent` and has no gate.

Hand-written `ServiceComponent` impls get the same hook: `fn enabled(&self)`
defaults to `true`, and `fn enabled_gate() -> Option<&'static str>` supplies
the label for the log line.

### `services.enabled` — the global off switch

The profile-level counterpart, and the reason a test boot does not have to
duplicate the app blueprint minus its workers:

```yaml
# application-test.yaml
services:
  enabled: false
```

`false` keeps **every** background service out of `run()`, on both spawn paths.
It composes with the per-service gate — a service runs only when the global
switch *and* its own `enabled()` both say yes — and defaults to `true` when the
key is absent (opt-out, never opt-in). It skips exactly what the per-service
gate skips and nothing more: registration, dependency resolution,
`from_context` and config validation all still run, so a test with services off
still fails on a broken service configuration.

There is **no profile sniffing**: it is an ordinary config key
(`r2e_core::runtime::service::SERVICES_ENABLED_KEY = "services.enabled"`,
read through `services_enabled(Option<&R2eConfig>)`), so it can equally be
flipped from `R2E_SERVICES_ENABLED` or
`override_config_value("services.enabled", false)`. The framework logs one
`info!` per process — not one per service — naming the key.

Where it is read: `AppBuilder::try_spawn_service_impl` captures it from the
builder's own config (`shared.config`), and `BeanRegistry::register_service_source`
(the `#[producer(start)]` / bean-declared path) reads the `R2eConfig` bean out
of the graph. Both evaluate it inside the service task, at the same moment the
per-service gate is read.

## You do NOT need an adapter struct

A recurring anti-pattern in consumer apps: a shared crate declares the worker
with plain fields, and the app crate wraps it in a second struct whose only
job is to hold `#[inject]` fields and clone them into the worker, because
`#[derive(BackgroundService)]` rejects fields it cannot resolve from the
graph. One adapter per worker, all identical, all noise.

`#[producer(start)]` is the answer. It registers its output as a bean **and**
runs it as a background service, so the worker keeps its plain constructor and
the app builds it from beans and config in one function:

```rust
// ── shared crate: no DI, no attributes ──────────────────────────────────
#[derive(Clone)]
pub struct Reindexer { sink: Sink, batch_size: usize }

impl Reindexer {
    pub fn new(sink: Sink, batch_size: usize) -> Self { Self { sink, batch_size } }
    pub async fn run(&self, shutdown: CancelToken) { /* loop */ }
}

// The produced value IS a bean, so the service reads itself back out of the
// graph. Three lines, and they can live in the shared crate too.
impl ServiceComponent for Reindexer {
    type Deps = TCons<Reindexer, TNil>;
    fn from_context(ctx: &BeanContext) -> Self { ctx.get::<Reindexer>() }
    async fn start(self, shutdown: CancelToken) { self.run(shutdown).await }
}

// ── app crate: one function, no adapter struct ──────────────────────────
#[producer(start)]
fn reindexer(sink: Sink, config: R2eConfig) -> Reindexer {
    Reindexer::new(sink, config.get_or("search.batch", 500))
}
```

The producer's own parameters are ordinary bean dependencies, so a missing one
is a compile error at `.register::<Reindexer_>()`, and the config keys the
producer declares are validated during `build_state()`. `after(...)` composes
with it when the worker must start behind another bean.

Use the derive when the worker's fields *are* the injected beans; use
`#[producer(start)]` when the worker is someone else's type with a plain
constructor.

## Cookbook — pick the right primitive

| Goal | Use |
|---|---|
| Slow side-task triggered by an HTTP request, fire-and-forget | `executor.submit_detached(...)` directly |
| Slow side-task whose result the handler awaits later | `#[async_exec]` returning `Result<JobHandle<T>, RejectedError>` |
| Periodic / event-driven worker bound to app lifecycle | `#[derive(BackgroundService)]` + `spawn_service::<C>()` |
| Worker type from a shared crate, with a plain constructor | `#[producer(start)]` — build it from beans/config, no adapter struct |
| A service the operator can turn off | `#[service(enabled = "…")]` (registration + config validation stay unconditional) |
| Every service off for a profile (tests) | `services.enabled: false` in `application-<profile>.yaml` |
| Startup work that must not repeat on an `r2e dev` hot patch | `.on_start_once(...)` / `#[on_start(once)]` |
| Cron / interval schedule | `#[scheduled]` — requires the `Executor` plugin; ticks run on this pool (drained, bounded, metered) |
| Submit work from inside a background service | Inject `PoolExecutor` and call `submit*` |

## Tests

`r2e-executor/tests/` exercises:

- `executor.rs` — `submit_and_await`, `concurrent_limit_enforced_by_semaphore`,
  `try_submit_rejects_when_queue_full`, `graceful_shutdown_drains_running_jobs`,
  `shutdown_aborts_queued_submissions`.
- `bg_service.rs` — `#[derive(BackgroundService)]` round-trip.
- `async_exec.rs` — `#[async_exec]` codegen returning `Result<JobHandle<T>, RejectedError>`,
  on both a `#[routes]` controller and a `#[bean]` impl.

See `examples/example-executor` for a runnable demo combining all three
primitives behind HTTP endpoints.
