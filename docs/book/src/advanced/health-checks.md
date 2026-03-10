# Health Checks

R2E provides two levels of health check support: a simple endpoint for basic deployments, and an advanced system with custom indicators, liveness/readiness probes, and result caching.

## Simple health check

Install the `Health` plugin for a minimal `GET /health` endpoint that always returns `"OK"` with status 200:

```rust
AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .with(Health)
    .serve("0.0.0.0:3000")
    .await;
```

This is sufficient for basic load balancer health checks where you only need to confirm the process is alive and accepting connections.

## Advanced health checks

For production systems that need to verify database connectivity, external service availability, or other dependencies, use `Health::builder()` to create an `AdvancedHealth` plugin with custom indicators.

```rust
use r2e::prelude::*;
use r2e::health::{HealthBuilder, HealthIndicator, HealthStatus};

AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .with(
        Health::builder()
            .check(DbHealth::new(pool.clone()))
            .check(RedisHealth::new(redis.clone()))
            .cache_ttl(Duration::from_secs(5))
            .build()
    )
    .serve("0.0.0.0:3000")
    .await;
```

### Endpoints

The advanced plugin registers three endpoints:

| Path | Behavior | Use case |
|------|----------|----------|
| `GET /health` | Runs all checks. Returns 200 with JSON if all are UP, 503 if any are DOWN. | General health monitoring, dashboards. |
| `GET /health/live` | Always returns 200 `"OK"`. No checks are run. | Kubernetes liveness probe. Confirms the process is running. |
| `GET /health/ready` | Runs checks where `affects_readiness()` returns `true`. Returns 200 if all pass, 503 otherwise. | Kubernetes readiness probe. Confirms the app can serve traffic. |

### Response format

The `/health` and `/health/ready` endpoints return JSON:

```json
{
  "status": "UP",
  "checks": [
    {
      "name": "db",
      "status": "UP",
      "duration_ms": 2
    },
    {
      "name": "redis",
      "status": "DOWN",
      "reason": "Connection refused",
      "duration_ms": 1003
    }
  ],
  "uptime_seconds": 3842
}
```

The top-level `status` is `"UP"` only when every individual check is UP. If any check is DOWN, the top-level status is `"DOWN"` and the HTTP status code is 503.

## The HealthIndicator trait

Implement `HealthIndicator` to define a custom health check:

```rust
use r2e::health::{HealthIndicator, HealthStatus};

struct DbHealth {
    pool: sqlx::SqlitePool,
}

impl DbHealth {
    fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }
}

impl HealthIndicator for DbHealth {
    fn name(&self) -> &str {
        "db"
    }

    async fn check(&self) -> HealthStatus {
        match sqlx::query("SELECT 1").fetch_one(&self.pool).await {
            Ok(_) => HealthStatus::Up,
            Err(e) => HealthStatus::Down(e.to_string()),
        }
    }
}
```

The trait has three methods:

| Method | Required | Description |
|--------|----------|-------------|
| `name(&self) -> &str` | Yes | A short identifier for this check (e.g. `"db"`, `"redis"`, `"disk"`). |
| `check(&self) -> impl Future<Output = HealthStatus>` | Yes | Performs the health check. Return `HealthStatus::Up` or `HealthStatus::Down(reason)`. |
| `affects_readiness(&self) -> bool` | No (default: `true`) | Whether this check is included in the `/health/ready` probe. |

### Liveness-only checks

Some checks indicate degraded state but should not prevent the application from receiving traffic. Override `affects_readiness` to exclude them from the readiness probe:

```rust
struct DiskSpaceHealth {
    threshold_mb: u64,
}

impl HealthIndicator for DiskSpaceHealth {
    fn name(&self) -> &str {
        "disk"
    }

    async fn check(&self) -> HealthStatus {
        let available = get_available_disk_mb();
        if available > self.threshold_mb {
            HealthStatus::Up
        } else {
            HealthStatus::Down(format!("Only {} MB available", available))
        }
    }

    fn affects_readiness(&self) -> bool {
        false // disk space issues should not block traffic
    }
}
```

With this configuration:
- `GET /health` includes the disk check in its response
- `GET /health/ready` skips it entirely

## HealthBuilder

`HealthBuilder` assembles indicators into an `AdvancedHealth` plugin:

```rust
use r2e::health::HealthBuilder;
use std::time::Duration;

let health_plugin = HealthBuilder::new()
    .check(DbHealth::new(pool.clone()))
    .check(RedisHealth::new(redis.clone()))
    .check(DiskSpaceHealth { threshold_mb: 500 })
    .cache_ttl(Duration::from_secs(10))
    .build();
```

| Method | Description |
|--------|-------------|
| `check(indicator)` | Register a `HealthIndicator`. Can be called multiple times. |
| `cache_ttl(duration)` | Cache health check results for the given duration. Subsequent requests within the TTL window reuse cached results instead of re-running checks. |
| `build()` | Consume the builder and produce an `AdvancedHealth` plugin. |

You can also access the builder through `Health::builder()` as a convenience:

```rust
Health::builder()
    .check(DbHealth::new(pool))
    .build()
```

## Cache TTL

Health checks that contact external services can be expensive. The `cache_ttl` option prevents checks from running on every request:

```rust
Health::builder()
    .check(DbHealth::new(pool))
    .cache_ttl(Duration::from_secs(5))
    .build()
```

With a 5-second TTL:
- The first request runs all checks and caches the result.
- Requests within the next 5 seconds return the cached response instantly.
- After 5 seconds, the next request re-runs all checks and refreshes the cache.

Both `/health` and `/health/ready` share the same cache. If no `cache_ttl` is set, every request runs all checks.

## Complete example

```rust
use r2e::prelude::*;
use r2e::health::{HealthBuilder, HealthIndicator, HealthStatus};
use std::time::Duration;

// -- Database health check --

struct DbHealth {
    pool: sqlx::SqlitePool,
}

impl HealthIndicator for DbHealth {
    fn name(&self) -> &str { "db" }

    async fn check(&self) -> HealthStatus {
        match sqlx::query("SELECT 1").fetch_one(&self.pool).await {
            Ok(_) => HealthStatus::Up,
            Err(e) => HealthStatus::Down(e.to_string()),
        }
    }
}

// -- External API health check --

struct ExternalApiHealth {
    url: String,
    client: reqwest::Client,
}

impl HealthIndicator for ExternalApiHealth {
    fn name(&self) -> &str { "external-api" }

    async fn check(&self) -> HealthStatus {
        match self.client.get(&self.url).send().await {
            Ok(resp) if resp.status().is_success() => HealthStatus::Up,
            Ok(resp) => HealthStatus::Down(format!("HTTP {}", resp.status())),
            Err(e) => HealthStatus::Down(e.to_string()),
        }
    }

    fn affects_readiness(&self) -> bool {
        false // external API being down should not block our readiness
    }
}

// -- Application setup --

#[tokio::main]
async fn main() {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .unwrap();

    let health = Health::builder()
        .check(DbHealth { pool: pool.clone() })
        .check(ExternalApiHealth {
            url: "https://api.example.com/health".into(),
            client: reqwest::Client::new(),
        })
        .cache_ttl(Duration::from_secs(10))
        .build();

    AppBuilder::new()
        .build_state::<AppState, _, _>()
        .await
        .with(Tracing)
        .with(health)
        .serve("0.0.0.0:3000")
        .await;
}
```
