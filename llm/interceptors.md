---
topic: interceptors
features: utils
tokens: ~1300
requires: guards
---

## Interceptors

### TL;DR

- Apply with `#[intercept(Expr)]` on a method or on the whole `#[routes]` impl
  block; an interceptor sees the handler's **raw return type** (`Json<T>`,
  `Result<…>`), never a `Response`.
- Built-ins from `r2e-utils`: `Logged`, `Timed`, `Counted`, `MetricTimed`,
  `Cache`, `CacheInvalidate`.
- `Cache` / `CacheInvalidate` need the store bean
  `.provide(InMemoryStore::shared())` (feature `cache`) — a missing store is a
  compile error — and a `Cacheable` return type.
- Put decorators **below** `#[routes]` / `#[bean]`; placing one above the
  transforming macro is a compile error.
- Impl-block decorators are **cumulative**, not replacing: controller-level
  checks run before the method-level ones.
- A controller-level guard/pre-guard is built **once** and shared by every
  route (its context reports `method_name: "*"`), so a stateful spec uses one
  bucket for the whole controller.
- `#[anonymous]` routes skip controller `#[guard]`/`#[roles]`/`#[all_roles]`
  but still run controller `#[pre_guard]`s and interceptors.
- Custom interceptor: `impl SelfBuilt` + `impl<R: Send> Interceptor<R>` with
  `around(&self, ctx, next)`; call `next().await` exactly where the wrapped
  body belongs.
- On `#[grpc_routes]` impl blocks only `#[intercept]` is accepted — the guard
  family is a compile error there.
- Full order: controller pre-guards → method pre-guards → identity extraction →
  controller guards → method guards → validation → controller interceptors →
  method interceptors → body.

Cross-cutting concerns via `#[intercept(...)]` on methods or the whole impl
block. Interceptors see the handler's **raw return type** (`Json<T>`,
`Result<...>`), never `Response`. Guards run first.

### Built-in (from `r2e-utils`)

```rust,ignore
#[intercept(Logged::info())]                  // log entry/exit
#[intercept(Timed::threshold(100))]           // only log if > 100ms
#[intercept(Counted::new("user_list_total"))] // named counter metric
#[intercept(MetricTimed::new("user_list_duration"))]
#[intercept(Cache::ttl(30))]                  // cache response 30s
#[intercept(Cache::ttl(60).group("users"))]   // named cache group
#[intercept(CacheInvalidate::group("users"))] // clear cache group
```

`Cache`/`CacheInvalidate` need the store bean: `.provide(InMemoryStore::shared())`
(feature `cache`). A missing store is a compile error. `Cache` requires the
return type to be `Cacheable` (`Json<T>`, `Result<T: Cacheable, E>`, or
`#[derive(Cacheable)]`).

### Block-level (controller-level decorators)

The whole decorator family is allowed on the `#[routes]` impl block —
`#[intercept]`, `#[guard]`, `#[pre_guard]`, `#[roles]`, `#[all_roles]`:

```rust,ignore
#[routes]
#[intercept(Logged::info())]                  // applies to all routes below
#[guard(RequireApiKey("x-api-key"))]          // every non-#[anonymous] route
#[pre_guard(PreRateLimit::per_ip(100, 60))]   // every route, #[anonymous] included
#[roles("member")]                            // every non-#[anonymous] route
impl UserController { ... }
```

Semantics:

- **Cumulative, not replacing** — controller checks run **before** the
  method-level ones (controller pre-guards → method pre-guards → identity →
  controller guards/roles → method guards/roles).
- **One shared instance** — a controller-level guard/pre-guard is built
  **once** per controller and shared by all its routes; its context reports
  `method_name: "*"`. A stateful spec (e.g. a rate limit) therefore uses a
  single bucket for the whole controller, unlike the per-route bucket it gets
  at method level.
- **`#[anonymous]` opts out of the post-auth half** — anonymous routes skip
  controller `#[guard]`/`#[roles]`/`#[all_roles]` (they run with no identity)
  but keep controller `#[pre_guard]`s and interceptors.
- Placement matters: decorators go **below** `#[routes]` (or `#[bean]`).
  Placing one **above** the transforming macro is a compile error (it would
  otherwise be silently dropped — attribute macros expand top-down). Any other
  route-family attribute on the impl block (`#[anonymous]`, `#[middleware]`,
  …) is also a targeted compile error, and a controller-level guard whose impl
  block has no eligible route is rejected as dead configuration.
- gRPC: `#[grpc_routes]` impl blocks accept `#[intercept]` only; the guard
  family there is a compile error.

### Custom interceptor

```rust
pub struct AuditLog;
impl SelfBuilt for AuditLog {}

impl<R: Send> Interceptor<R> for AuditLog {
    async fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> R
    where F: FnOnce() -> Fut + Send, Fut: Future<Output = R> + Send {
        tracing::info!(method = ctx.method_name, "audit: entering");
        let result = next().await;
        tracing::info!(method = ctx.method_name, "audit: done");
        result
    }
}
```

Bean deps → `#[derive(DecoratorBean)]`, same as guards. See llm/guards.md.

Execution order: controller-level pre-auth guards → method-level pre-auth
guards → identity extraction → controller-level guards → method-level guards
(declaration order within each level) → validation → controller-level
interceptors → method-level interceptors → body.
