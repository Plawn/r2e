# r2e-rate-limit

Token-bucket rate limiting for R2E — per-user, per-IP, or global rate limits.

## Overview

Provides a generic token-bucket rate limiter with pluggable backends and declarative guard integration. Supports both pre-auth (before JWT validation) and post-auth (after identity extraction) rate limiting.

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.1", features = ["rate-limit"] }
```

## Declarative usage

### Pre-auth rate limiting (no identity required)

```rust
use r2e::r2e_rate_limit::PreRateLimit;

#[routes]
impl MyController {
    #[get("/")]
    #[pre_guard(PreRateLimit::global(100, 60))]   // 100 requests / 60 seconds, shared bucket
    async fn public_endpoint(&self) -> &'static str { "ok" }

    #[post("/login")]
    #[pre_guard(PreRateLimit::per_ip(5, 60))]     // 5 requests / 60 seconds, per IP
    async fn login(&self) -> &'static str { "ok" }
}
```

### Post-auth rate limiting (requires identity)

```rust
#[routes]
impl MyController {
    #[get("/api/data")]
    #[guard(RateLimit::per_user(30, 60))]      // 30 requests / 60 seconds, per user
    async fn user_data(&self) -> Json<Data> { ... }
}
```

### Config-resolved budgets

```rust
use r2e::r2e_rate_limit::{ConfiguredPreRateLimit, ConfiguredRateLimit};

#[post("/start")]
#[pre_guard(ConfiguredPreRateLimit::per_ip("rate-limit.public").defaults(30, 60))]
async fn start(&self) -> &'static str { "ok" }
```

```yaml
rate-limit:
  public:
    max: 30
    window-secs: 60
    enabled: true              # false → always allow
    trust-forwarded-for: true  # false → peer address only
```

The defaults apply **only when a key is absent**. A present-but-invalid value
(`max: plenty`) — or `window-secs: 0` — aborts startup rather than silently
reinstating the default budget.

## Bucket keys

`<module::path::ControllerName>:<handler>:{global | ip:<client-ip> | user:<sub>}`
— each annotated handler owns its bucket, and the controller name is
module-qualified, so neither homonymous handlers nor same-named controllers in
different modules share one.

Client IP for `per_ip`: leftmost `X-Forwarded-For` entry **that parses as an IP
address** (port stripped, IPv6 canonicalized) → transport peer address
(`ConnectInfo<SocketAddr>`) → `unknown` (warn-once). A malformed entry is treated
as absent, so junk can neither mint a bucket nor suppress the peer fallback.
`X-Forwarded-For` is forgeable unless the proxy overwrites it; with no proxy in
front use `PreRateLimit::per_ip(..).peer_ip_only()` (or
`trust-forwarded-for: false`).

## Contracts

- `window_secs` must be > 0 everywhere: literal constructors and `.defaults(..)`
  panic, `<prefix>.window-secs: 0` aborts startup. A zero window would refill the
  bucket on every request.
- Per-user limits require an identity (`DecoratorSpec::REQUIRES_IDENTITY = true`):
  a compile error where the route can never have one, `401 Unauthorized` at
  runtime when an optional identity is `None`. Unauthenticated callers never
  share an "anonymous" bucket.

## Key types

### RateLimiter

Generic token-bucket rate limiter keyed by arbitrary type:

```rust
use r2e::r2e_rate_limit::RateLimiter;
use std::time::Duration;

let limiter = RateLimiter::new(10, Duration::from_secs(60)); // 10 tokens per 60 seconds
if limiter.try_acquire(&"user-123") {
    // request allowed
}
```

### RateLimitBackend

Pluggable backend trait (`fn try_acquire(&self, key: &str, max: u64, window_secs: u64) -> bool`).
Default: `InMemoryRateLimiter` (DashMap-backed) — **single-process**, so N
replicas allow up to N × `max` per window; implement the trait over a shared
store (Redis, …) for a cluster-wide limit.

### RateLimitRegistry

Clonable handle stored in app state, managing rate limiter instances for the generated guards.

## Key classification

| Key kind | Guard type | When to use |
|----------|-----------|-------------|
| `PreRateLimit::global()` | `#[pre_guard]` | Shared bucket, before JWT validation |
| `PreRateLimit::per_ip()` | `#[pre_guard]` | Per client IP (XFF → peer address), before JWT validation |
| `RateLimit::per_user()` | `#[guard]` | Per authenticated user, after JWT validation |
| `ConfiguredPreRateLimit::{global,per_ip}()` | `#[pre_guard]` | Same, budget from config |
| `ConfiguredRateLimit::per_user()` | `#[guard]` | Same, budget from config |

## License

Apache-2.0
