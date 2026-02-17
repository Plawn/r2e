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
use r2e::r2e_rate_limit::RateLimit;

#[routes]
impl MyController {
    #[get("/")]
    #[pre_guard(RateLimit::global(100, 60))]   // 100 requests / 60 seconds, shared bucket
    async fn public_endpoint(&self) -> &'static str { "ok" }

    #[post("/login")]
    #[pre_guard(RateLimit::per_ip(5, 60))]     // 5 requests / 60 seconds, per IP
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

## Key types

### RateLimiter

Generic token-bucket rate limiter keyed by arbitrary type:

```rust
use r2e::r2e_rate_limit::RateLimiter;

let limiter = RateLimiter::new(10, 60); // 10 tokens per 60 seconds
if limiter.check("user-123") {
    // request allowed
}
```

### RateLimitBackend

Pluggable backend trait. Default: `InMemoryRateLimiter` (DashMap-backed).

### RateLimitRegistry

Clonable handle stored in app state, managing rate limiter instances for the generated guards.

## Key classification

| Key kind | Guard type | When to use |
|----------|-----------|-------------|
| `RateLimit::global()` | `#[pre_guard]` | Shared bucket, before JWT validation |
| `RateLimit::per_ip()` | `#[pre_guard]` | Per X-Forwarded-For, before JWT validation |
| `RateLimit::per_user()` | `#[guard]` | Per authenticated user, after JWT validation |

## License

Apache-2.0
