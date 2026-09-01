---
topic: guards
features: core, rate-limit
tokens: ~2500
requires: security
---

## Guards

### TL;DR

- `#[guard(Expr)]` is evaluated **once at controller registration**, never per
  request — a guard cannot read the app state at request time.
- No bean deps: `impl SelfBuilt for MyGuard {}` + `impl<I: Identity> Guard<I>`;
  the expression itself is the guard (tuple structs work: `#[guard(RequireApiKey("x-api-key"))]`).
- Bean or config deps: `#[derive(DecoratorBean)]` with `#[inject]` / `#[config]`
  fields, applied as `MyGuard::spec(args)` (plain fields, declaration order). A
  missing bean is a compile error at `register_controller()`.
- Deny with `Err(GuardError::forbidden(…).into())` / `unauthorized` /
  `new(status, msg)`; read the request through `GuardContext`.
- Checks that must run **before** JWT extraction implement `PreAuthGuard` and
  are applied with `#[pre_guard(…)]`.
- Rate limiting needs feature `rate-limit` and `.provide(RateLimitRegistry::default())`;
  `PreRateLimit` is pre-auth, `RateLimit::per_user` is post-auth.
- `window_secs` must be > 0 (every constructor panics on 0), and a per-user
  limit on a route that can never have an identity is a compile error.
- YAML-tunable budgets use the separate `ConfiguredPreRateLimit` /
  `ConfiguredRateLimit` specs with `.defaults(max, window)` — an invalid
  configured value aborts startup instead of falling back.
- With no trusted proxy in front, use `.peer_ip_only()` — `X-Forwarded-For` is
  forgeable unless the proxy overwrites it.
- `InMemoryRateLimiter` is single-process; implement `RateLimitBackend` over a
  shared store for a cluster-wide limit.

Guards are **graph-resolved decorators**: the `#[guard(...)]` expression is
evaluated once at controller registration; bean deps are declared at the type
level and compile-checked (a missing bean is a compile error at
`register_controller()`). No state access at request time.

### Simple guard (no bean deps) — `SelfBuilt`

```rust
pub struct BlockUser;
impl SelfBuilt for BlockUser {}            // one line: the expression IS the guard

impl<I: Identity> Guard<I> for BlockUser {
    async fn check(&self, ctx: &GuardContext<'_, I>) -> Result<(), Response> {
        if ctx.identity_sub() == Some("blocked-user") {
            return Err(GuardError::forbidden("Blocked").into());
        }
        Ok(())
    }
}

// usage:
#[controller(path = "/data")]
pub struct DataController;

#[routes]
impl DataController {
    #[get("/protected")]
    #[guard(BlockUser)]
    async fn protected(&self) -> Json<Data> { Json(Data::default()) }

    // tuple-struct config works directly:
    #[get("/keyed")]
    #[guard(RequireApiKey("x-api-key"))]
    async fn keyed(&self) -> Json<Data> { Json(Data::default()) }
}
# fn main() {}
```

`GuardContext` provides: `method_name`, `controller_name`, `method`, `headers`,
`uri` (`.path()`, `.query_string()`), `extensions`, `peer_addr`, `path_params`
(`path_param()`, `parse_path_param()`), `identity: Option<&I>`, plus
`identity_sub()`, `identity_email()`, `identity_claims()`, and `head()` (the
same `RequestHead` view `#[managed]` resources get). Building one by hand (rare
— outside a request) can use `default_method()` / `no_extensions()`.
Error helper: `GuardError::forbidden(...)`,
`GuardError::unauthorized(...)`, `GuardError::new(status, msg)`.

### Bean-reading guard — `#[derive(DecoratorBean)]`

```rust
#[derive(DecoratorBean)]
pub struct ProjectGuard {
    #[inject] pool: PgPool,                 // from the bean graph (compile-checked)
    #[config("app.tenant")] tenant: String, // from R2eConfig
    min_role: &'static str,                 // plain field = site config
}

impl<I: Identity> Guard<I> for ProjectGuard {
    async fn check(&self, ctx: &GuardContext<'_, I>) -> Result<(), Response> {
        Ok(())                              // uses self.pool / self.tenant / self.min_role
    }
}

// at the site — plain fields in declaration order:
#[controller(path = "/projects")]
pub struct ProjectController;

#[routes]
impl ProjectController {
    #[get("/")]
    #[guard(ProjectGuard::spec("viewer"))]
    async fn list(&self) -> Json<Vec<Data>> { Json(Vec::new()) }
}
# fn main() {}
```

`#[config]` / `#[config_section]` / `#[live_config]` fields on a decorator bean
are **declared** (`DecoratorSpec::config_keys()` for keys and
`DecoratorSpec::config_sections()` for `#[config_section]` fields — a
`SectionValidator` per field, which walks the whole section: missing nested
keys, type mismatches, `garde` rules — both emitted by the derive) and
aggregated by whichever host owns the site — a `#[routes]` controller
(`Controller::validate_config`, checked at `register_controller`), a `#[bean]`
impl with `#[intercept]` on `#[scheduled]`/`#[consumer]` methods (folded into
`Bean::config_keys()`), or a `#[grpc_routes]` service
(`GrpcService::validate_config`, checked at `register_grpc_service`). A missing
required key is therefore part of the aggregated startup configuration report
that names every missing key at once, not a late panic when the guard is built.

### Pre-auth guards

Run as middleware **before** JWT extraction (IP allowlists, pre-auth rate
limits). Implement `PreAuthGuard` (context: `PreAuthGuardContext`, no identity),
apply with `#[pre_guard(MyPreGuard)]`. Supported on SSE and WS routes too.

### Rate limiting

Requires feature: `rate-limit`. Provide the registry: `.provide(RateLimitRegistry::default())`.

```rust
use r2e::r2e_rate_limit::{PreRateLimit, RateLimit};

#[controller(path = "/reports")]
pub struct ReportController {
    #[inject(identity)] user: AuthenticatedUser,
}

#[routes]
impl ReportController {
    #[get("/")]
    #[pre_guard(PreRateLimit::global(100, 60))]   // 100 req / 60s, shared bucket (pre-auth)
    #[pre_guard(PreRateLimit::per_ip(10, 60))]    // per IP (pre-auth)
    #[pre_guard(PreRateLimit::per_ip(10, 60).peer_ip_only())] // ignore X-Forwarded-For
    #[guard(RateLimit::per_user(5, 60))]          // per user (post-auth, needs identity)
    async fn list(&self) -> Json<Vec<Data>> { Json(Vec::new()) }
}
# fn main() {}
```

`window_secs` must be > 0 — a zero window refills the bucket on every request,
so every constructor panics on it.

Per-user limits (`RateLimit` / `ConfiguredRateLimit`) set
`DecoratorSpec::REQUIRES_IDENTITY = true`: putting one on a route that can never
have an identity (no struct-level `#[inject(identity)]`, no identity param, or
`#[anonymous]` without an `Option<..>` identity param) is a **compile error**,
and an `Option<..>` identity that is `None` at runtime gets **401**, never a
shared "anonymous" bucket.

Bucket keys are `<module::path::ControllerName>:<handler>:{global|ip:<ip>|user:<sub>}`
— each annotated handler owns its bucket, and the controller name is
module-qualified, so neither homonymous handlers nor same-named controllers in
different modules share one.

Client IP for `per_ip`: leftmost `X-Forwarded-For` entry **when it parses as an
IP address**, else the transport peer address (`ConnectInfo<SocketAddr>`, port
stripped), else the shared `unknown` bucket (warned once via `tracing::warn!`).
Accepted entry forms: `1.2.3.4`, `1.2.3.4:5678`, `2001:db8::1`, `[::1]`,
`[::1]:8080`; anything else (`unknown`, junk, empty) is treated as **absent**, so
it neither becomes a bucket key nor suppresses the peer fallback. The key uses
the canonical `IpAddr` `Display` form, so IPv6 aliases share one bucket.
`X-Forwarded-For` is forgeable unless the proxy **overwrites** it (nginx
`$remote_addr`, not `$proxy_add_x_forwarded_for`); with no proxy in front use
`.peer_ip_only()`. Guards can read `ctx.forwarded_for()` (raw `Option<&str>`,
unvalidated — do not key on it), `ctx.forwarded_ip()` (`Option<IpAddr>`, parsed),
`ctx.peer_ip()` (`Option<IpAddr>`), `ctx.client_ip()`
(`Option<ClientIp>`, where `ClientIp` is `Forwarded(IpAddr) | Peer(IpAddr)` with
`ip()` + `Display`) — on both `GuardContext` and `PreAuthGuardContext`;
`peer_addr: Option<SocketAddr>` is a public field, `None` under `TestApp`'s
in-process dispatch. `r2e_core::parse_forwarded_ip(&str) -> Option<IpAddr>` is
the same parser, for hand-written guards.

Config-tunable budgets — separate spec types (they additionally depend on
`R2eConfig`), literal args are the fallback defaults:

```rust
use r2e::r2e_rate_limit::{ConfiguredPreRateLimit, ConfiguredRateLimit};

#[controller(path = "/api")]
pub struct ApiController {
    #[inject(identity)] user: AuthenticatedUser,
}

#[routes]
impl ApiController {
    #[get("/")]
    #[pre_guard(ConfiguredPreRateLimit::per_ip("rate-limit.public").defaults(30, 60))]
    #[pre_guard(ConfiguredPreRateLimit::global("rate-limit.public").defaults(30, 60))]
    #[guard(ConfiguredRateLimit::per_user("rate-limit.api").defaults(5, 60))]
    async fn list(&self) -> Json<Vec<Data>> { Json(Vec::new()) }
}
# fn main() {}
```

```yaml
rate-limit:
  public:
    max: 30
    window-secs: 60
    enabled: true              # false → guard always allows (e.g. test profile)
    trust-forwarded-for: true  # false → peer address only
```

The `.defaults(max, window)` values apply **only when a key is absent**. A
present-but-invalid value (`max: plenty`, `enabled: sure`) or `window-secs: 0`
**aborts startup** with `invalid configuration for `<prefix>.<key>`` — a
malformed security budget never silently degrades to the default.

`InMemoryRateLimiter` is single-process: N replicas allow up to N × `max` per
window. Implement `RateLimitBackend` (`fn try_acquire(&self, key: &str, max: u64,
window_secs: u64) -> bool`, sync) over a shared store for a cluster-wide limit,
then `.provide(RateLimitRegistry::new(MyBackend))`.

### Do not

- Do not key anything on `ctx.forwarded_for()` — it is the raw, unvalidated
  header; use `ctx.forwarded_ip()` / `ctx.client_ip()`, or `.peer_ip_only()`
  when nothing overwrites `X-Forwarded-For` in front of the app.
- Do not count on `InMemoryRateLimiter` across replicas: N replicas allow up to
  N × `max` per window.
- Do not build a rate limit with `window_secs: 0`, and do not expect a
  malformed configured budget to fall back to `.defaults(...)` — it aborts
  startup.
