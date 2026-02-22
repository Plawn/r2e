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

let cache = TtlCache::new();
cache.insert("key", "value", Duration::from_secs(60));

if let Some(value) = cache.get(&"key") {
    println!("cached: {}", value);
}

cache.remove(&"key");
cache.evict_expired(); // clean up expired entries
```

### CacheStore

Pluggable async cache backend trait. Default implementation: `InMemoryStore` (DashMap-backed).

```rust
use r2e::r2e_cache::{CacheStore, InMemoryStore, set_cache_backend};

// Use the default in-memory store
set_cache_backend(InMemoryStore::new());

// Or implement CacheStore for your own backend (Redis, etc.)
```

Operations: `get`, `set`, `remove`, `clear`, `remove_by_prefix`.

### Interceptor integration

When used with [`r2e-utils`](../r2e-utils), caching can be applied declaratively:

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
