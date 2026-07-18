# r2e-cache

TTL cache with pluggable backends for R2E — in-memory caching with expiration.

## Overview

Provides a thread-safe TTL cache backed by `DashMap`, plus a pluggable `CacheStore` trait for custom backends. Zero framework dependencies — can be used standalone.

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.1", features = ["cache"] }
```

## Key types

### TtlCache

Thread-safe TTL cache:

```rust
use r2e::r2e_cache::TtlCache;
use std::time::Duration;

let cache = TtlCache::new(Duration::from_secs(60)); // TTL is fixed per cache
cache.insert("key", "value");

if let Some(value) = cache.get(&"key") {
    println!("cached: {}", value);
}

cache.remove(&"key");
cache.evict_expired(); // clean up expired entries
```

### CacheStore

Pluggable async cache backend trait. Default implementation: `InMemoryStore` (DashMap-backed).

The store is a **bean** (`Arc<dyn CacheStore>`). Provide one on the builder so
the `Cache` / `CacheInvalidate` interceptors can resolve it from the graph:

```rust
use r2e::r2e_cache::InMemoryStore;

AppBuilder::new()
    .provide(InMemoryStore::shared())   // Arc<dyn CacheStore>
    // ... other beans ...
    .build_state()
    .await;

// Or implement CacheStore for your own backend (Redis, etc.) and provide that.
```

> There is no global cache store — the old `cache_backend()` / `set_cache_backend()`
> functions have been removed. The store is always a bean.

Operations: `get`, `set`, `remove`, `clear`, `remove_by_prefix`.

### Interceptor integration

When used with [`r2e-utils`](../r2e-utils), caching can be applied declaratively.
The `Cache` and `CacheInvalidate` interceptors read the `Arc<dyn CacheStore>`
bean from the graph — a missing store is a compile error at `register_controller()`:

```rust
#[get("/")]
#[intercept(Cache::ttl(30).group("users"))]
async fn list(&self) -> Json<Vec<User>> { ... }

#[post("/")]
#[intercept(CacheInvalidate::group("users"))]
async fn create(&self, body: Json<CreateUser>) -> Result<Json<User>, HttpError> { ... }
```

## License

Apache-2.0
