# R2E Executor — Managed Task Pool & Background Services

The `r2e-executor` crate provides a managed task pool (`PoolExecutor`) and
ergonomic primitives for off-request work — analogous to JEE's
`ManagedExecutorService` and Quarkus's `@ApplicationScoped @Startup` services.

Three pieces:

1. `PoolExecutor` — bounded, semaphore-gated Tokio task pool. Injectable bean.
2. `#[async_exec]` — controller-method attribute that submits the body to the
   pool and returns a `Result<JobHandle<T>, RejectedError>` instead of `T`.
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

Marks a method on a `#[routes]` controller as a pool-executed job. The
generated wrapper:

- Takes the same arguments as the original method.
- Returns `Result<JobHandle<T>, RejectedError>` instead of `T`.
- Is **not** `async` — the synchronous handle resolves to the result.

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

    #[async_exec]                                     // default executor field: `executor`
    async fn generate_pdf(&self, id: u64) -> Vec<u8> {
        // ...heavy work...
        format!("PDF #{id}").into_bytes()
    }
}
```

Override the executor field with `#[async_exec(executor = "io_pool")]`.

**Constraints (compile-time):**

- The annotated method must be `async fn(&self, ...) -> T`.
- The controller must be `Clone + Send + Sync + 'static`
  (`#[controller]` already implies this).
- The named field must implement
  `r2e_executor::PoolExecutor`-compatible `submit(...)` — typically a
  `PoolExecutor` `#[inject]`ed bean.

**Codegen:** the original body is renamed
`__r2e_async_<method>_inner` and a synchronous wrapper takes its place,
cloning `self`, capturing the executor, and submitting an `async move`
block.

## `#[derive(BackgroundService)]`

Generates `impl ServiceComponent` (no state generic) from the same `#[inject]` /
`#[config]` field syntax used by `#[controller]`. The component is built from
the resolved bean graph via `from_context(&BeanContext)` — each `#[inject]`
field resolved by type. The user supplies an `async fn run(&self,
CancellationToken)` method; the derived `start` just forwards to it.

```rust
use tokio_util::sync::CancellationToken;

#[derive(BackgroundService, Clone)]
pub struct EmailWorker {
    #[inject] executor: PoolExecutor,
    #[inject] mailer: Mailer,
    #[config("email.batch_size")] batch_size: u64,
}

impl EmailWorker {
    async fn run(&self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            tokio::select! {
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

`#[derive(BackgroundService)]` takes **no** attribute — there is no
`#[service(state = ...)]`. The service resolves its `#[inject]` fields from the
bean graph by type (like a controller core), so it works with the inferred HList
state; each injected type must be present in the graph or `spawn_service::<C>()`
is a compile error naming the missing type.

## Cookbook — pick the right primitive

| Goal | Use |
|---|---|
| Slow side-task triggered by an HTTP request, fire-and-forget | `executor.submit_detached(...)` directly |
| Slow side-task whose result the handler awaits later | `#[async_exec]` returning `Result<JobHandle<T>, RejectedError>` |
| Periodic / event-driven worker bound to app lifecycle | `#[derive(BackgroundService)]` + `spawn_service::<C>()` |
| Cron / interval schedule | `#[scheduled]` — requires the `Executor` plugin; ticks run on this pool (drained, bounded, metered) |
| Submit work from inside a background service | Inject `PoolExecutor` and call `submit*` |

## Tests

`r2e-executor/tests/` exercises:

- `executor.rs` — `submit_and_await`, `concurrent_limit_enforced_by_semaphore`,
  `try_submit_rejects_when_queue_full`, `graceful_shutdown_drains_running_jobs`,
  `shutdown_aborts_queued_submissions`.
- `bg_service.rs` — `#[derive(BackgroundService)]` round-trip.
- `async_exec.rs` — `#[async_exec]` codegen returning `Result<JobHandle<T>, RejectedError>`.

See `examples/example-executor` for a runnable demo combining all three
primitives behind HTTP endpoints.
