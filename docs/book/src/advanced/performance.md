# Performance Guide

R2E is designed for minimal overhead. Understanding the cost model helps you make informed decisions.

## Per-request cost model

| Operation | When it runs | Notes |
|-----------|--------------|-------|
| Tower layers and Axum routing | Every request | Depends on the installed layer stack and route shape |
| Controller core `Arc` clone | Every request | One logical increment; the core itself is built once |
| Request façade binding | Every request | Stack construction; no allocation or DI lookup |
| `#[inject]` field clone | Registration only | Stored in the shared core |
| `#[config("key")]` lookup | Registration only | Stored in the shared core |
| Identity extraction/JWT validation | Only when requested | Usually dominates façade dispatch when cryptography is involved |
| Guards and interceptors | Only when configured | Monomorphized; cost depends on their implementation |
| Business logic | Every request | Usually dominated by application I/O |

## Reproducible dispatch benchmark

`r2e-core/benches/controller_dispatch.rs` compares in-process
`tower::oneshot` paths with identical handler work and responses. The comparable
Axum baselines are built through the same `AppBuilder`, so they include the same
global layers as the controller paths. A separate bare-Axum scenario is retained
only to expose the cost of that application stack. The identity extractor is a
stub, so JWT cryptography and network I/O are excluded:

```bash
cargo bench -p r2e-core --bench controller_dispatch -- \
  --warm-up-time 2 --measurement-time 5 --sample-size 100
```

A sample run on 2026-06-29 produced:

| Comparable scenario | Observed interval |
|---------------------|-------------------|
| Axum + application stack, captured `Arc` | 812–827 ns |
| R2E without request-scoped fields | 839–846 ns |
| Axum + application stack + stub identity | 848–859 ns |
| R2E parameter identity | 843–854 ns |
| R2E struct identity façade | 844–863 ns |

These are local micro-benchmark results, not latency guarantees. Re-run the
benchmark on the target hardware and toolchain before drawing conclusions from
small differences. In this sample, the standard controller adds about 24 ns
over its equivalent Axum route. The identity variants overlap within benchmark
noise; real JWT validation costs far more than this dispatch plumbing.

## Optimization guidelines

### 1. Use cheap-to-clone application dependencies

An injected dependency is cloned once when the controller core is registered,
not once per request. Prefer `Arc`-backed service handles when cloning the
service itself would duplicate substantial state:

```rust
#[derive(Clone)]
pub struct UserService {
    inner: Arc<UserServiceInner>,
}
```

### 2. Use param-level identity for mixed controllers

Struct-level `#[inject(identity)]` runs JWT validation for **every** endpoint — even public ones. Param-level only validates when needed:

```rust
// Bad: JWT validated for public endpoints too
#[controller(state = AppState)]
pub struct ApiController {
    #[inject(identity)] user: AuthenticatedUser,  // validates on ALL endpoints
}

// Good: JWT only validated where needed
#[controller(state = AppState)]
pub struct ApiController {
    #[inject] service: MyService,
}

#[routes]
impl ApiController {
    #[get("/public")]
    async fn public_data(&self) -> Json<Data> { /* no JWT overhead */ }

    #[get("/me")]
    async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<User> {
        Json(user)
    }
}
```

### 3. `#[config]` fields are resolved once

Each `#[config]` field is resolved into the controller core when the router is built, not per request — so they are effectively free at request time. There is no need to cache them in a service field.

### 4. Pre-warm JWKS cache

The first JWT validation with a JWKS endpoint incurs a ~50-200 ms HTTP roundtrip. Pre-warm in an `on_start` hook:

```rust
.on_start(|state| async move {
    // Trigger a JWKS cache refresh
    state.validator.refresh_jwks().await?;
    Ok(())
})
```

### 5. Keep interceptors focused

Interceptors use monomorphization without a virtual call, but their own work is
not free. Logging, serialization, cache access, and metrics should be evaluated
with the same care as equivalent code in a handler.

### 6. Guards should be O(1)

Guards run on every guarded request. Keep them fast:
- Role checks: O(n) where n = number of required roles (typically small)
- Rate limit checks: O(1) DashMap lookup
- Avoid database queries in guards when possible

### 7. One controller per responsibility

Avoid putting unrelated endpoints in the same controller. Each controller shares
the same injection profile; unnecessary services increase retained core state
and registration work.

## Anti-patterns

### Storing `R2eConfig` as `#[inject]`

```rust
// Usually too broad: clones the config handle into the core at registration
#[inject] config: R2eConfig,

// Good: use specific #[config] fields
#[config("app.name")] name: String,
```

### I/O in custom guards

```rust
// Bad: blocks the Tokio runtime
impl<S, I: Identity> Guard<S, I> for SlowGuard {
    fn check(&self, state: &S, ctx: &GuardContext<'_, I>) -> impl Future<...> + Send {
        async move {
            // This is a database query on EVERY request
            let result = sqlx::query("SELECT ...").fetch_one(&pool).await;
            // ...
        }
    }
}
```

If you must do I/O in guards, consider caching results or moving the check to the handler body.

## Tokio runtime tuning

`#[r2e::main]` exposes Tokio `runtime::Builder` options as macro attributes:

| Argument | Default | Description |
|----------|---------|-------------|
| `flavor` | `"multi_thread"` | `"multi_thread"` or `"current_thread"` |
| `worker_threads` | Tokio default | Number of async worker threads |
| `max_blocking_threads` | 512 | Max threads for `spawn_blocking` tasks |
| `thread_stack_size` | 2 MiB | Stack size per worker thread (bytes) |
| `thread_name` | `"tokio-runtime-worker"` | Worker thread name prefix |
| `global_queue_interval` | 31 | How often workers check the global queue |
| `event_interval` | 61 | Max events processed per scheduler tick |
| `thread_keep_alive` | 10 | Keep-alive for idle blocking threads (seconds) |

```rust
// Deep call stacks (e.g. recursive tree processing)
#[r2e::main(thread_stack_size = 8388608)]
async fn main() { /* 8 MiB stack per worker */ }

// CPU-bound workloads with many blocking tasks
#[r2e::main(worker_threads = 8, max_blocking_threads = 256)]
async fn main() { /* ... */ }

// Single-threaded for lightweight services
#[r2e::main(flavor = "current_thread")]
async fn main() { /* ... */ }
```

These are compile-time constants — the Tokio runtime is built before any async code (including config loading) runs. For runtime-variable tuning, build the runtime manually instead of using the macro.

## Comparison: struct-level vs param-level identity

| Aspect | Struct-level | Param-level |
|--------|-------------|------------|
| JWT validation | Every request | Only annotated endpoints |
| `StatefulConstruct` | Generated (core) | Generated (core) |
| Consumers/Schedulers | Possible (run on core) | Possible (run on core) |
| Identity access | `self.user` | Only in handler parameter |
| Public endpoint overhead | JWT validation wasted | None |

Both forms build the controller **core** once at registration; neither
reconstructs the core's dependencies per request. The difference is purely about
*when* identity is extracted: struct-level identity extracts (and validates) an
identity for every endpoint into the per-request façade, while parameter-level
identity extracts only on the endpoints that ask for it. Prefer parameter-level
identity for mixed public/protected controllers so public endpoints pay no JWT
cost. See [Controller Lifecycle and
Handler Dispatch](./controller-lifecycle-and-dispatch.md) for the generated
dispatch path.
