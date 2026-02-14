# Rate Limiting

R2E provides token-bucket rate limiting with three key strategies: global, per-IP, and per-user.

## Setup

Enable the rate-limit feature:

```toml
r2e = { version = "0.1", features = ["rate-limit"] }
```

Add `RateLimitRegistry` to your state:

```rust
use r2e::r2e_rate_limit::RateLimitRegistry;

#[derive(Clone, BeanState)]
pub struct AppState {
    pub rate_limiter: RateLimitRegistry,
    // ...
}
```

Provide a default instance:

```rust
AppBuilder::new()
    .provide(RateLimitRegistry::default())
    // ...
```

## Rate limit strategies

### Global rate limit (pre-auth)

Shared bucket across all requests. Runs before JWT validation:

```rust
use r2e::r2e_rate_limit::RateLimit;

#[get("/")]
#[pre_guard(RateLimit::global(100, 60))]  // 100 requests per 60 seconds total
async fn list(&self) -> Json<Vec<Item>> { /* ... */ }
```

### Per-IP rate limit (pre-auth)

Separate bucket per client IP. Runs before JWT validation:

```rust
#[get("/")]
#[pre_guard(RateLimit::per_ip(10, 60))]  // 10 requests per 60 seconds per IP
async fn list(&self) -> Json<Vec<Item>> { /* ... */ }
```

IP is extracted from the `X-Forwarded-For` header.

### Per-user rate limit (post-auth)

Separate bucket per authenticated user. Runs after JWT validation:

```rust
#[post("/")]
#[guard(RateLimit::per_user(5, 60))]  // 5 requests per 60 seconds per user
async fn create(&self, body: Json<Request>) -> Json<Response> { /* ... */ }
```

User is identified by the `sub` claim from the JWT token.

## Key classification

| Strategy | Attribute | Runs when | Needs identity |
|----------|-----------|-----------|---------------|
| `RateLimit::global(max, window)` | `#[pre_guard(...)]` | Before JWT | No |
| `RateLimit::per_ip(max, window)` | `#[pre_guard(...)]` | Before JWT | No |
| `RateLimit::per_user(max, window)` | `#[guard(...)]` | After JWT | Yes |

## Combining rate limits

```rust
#[post("/upload")]
#[pre_guard(RateLimit::global(1000, 60))]     // 1000 total uploads/min
#[pre_guard(RateLimit::per_ip(50, 60))]       // 50 uploads/min per IP
#[guard(RateLimit::per_user(10, 60))]         // 10 uploads/min per user
async fn upload(&self, body: Bytes) -> Result<(), AppError> { /* ... */ }
```

## Response on rate limit exceeded

When a rate limit is exceeded, R2E returns:

```
HTTP/1.1 429 Too Many Requests
```

## Custom rate limit backend

The default backend is `InMemoryRateLimiter` (DashMap-based). For distributed rate limiting, implement the `RateLimitBackend` trait:

```rust
use r2e_rate_limit::RateLimitBackend;

struct RedisRateLimiter { /* ... */ }

impl RateLimitBackend for RedisRateLimiter {
    async fn check_rate_limit(
        &self,
        key: &str,
        max_requests: u64,
        window_secs: u64,
    ) -> bool {
        // Return true if allowed, false if exceeded
        todo!()
    }
}
```
