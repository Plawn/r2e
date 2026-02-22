# r2e-utils

Built-in interceptors for R2E — Logged, Timed, Cache, and CacheInvalidate.

## Overview

Provides ready-to-use interceptors that implement the `Interceptor<R>` trait from `r2e-core`. All calls are monomorphized at compile time for zero overhead.

## Usage

Via the facade crate (enabled by default):

```toml
[dependencies]
r2e = "0.1"  # utils is a default feature
```

## Interceptors

### Logged

Logs method entry and exit at a configurable log level:

```rust
#[get("/")]
#[intercept(Logged::info())]
async fn list(&self) -> Json<Vec<User>> { ... }

// Also available: Logged::debug(), Logged::warn(), Logged::error(), Logged::trace()
```

### Timed

Measures execution time, with an optional threshold (only logs if exceeded):

```rust
#[get("/")]
#[intercept(Timed::new())]                  // always logs duration
async fn list(&self) -> Json<Vec<User>> { ... }

#[get("/slow")]
#[intercept(Timed::threshold_ms(100))]      // only logs if > 100ms
async fn slow(&self) -> Json<Data> { ... }
```

### Cache

Caches `Json<T>` responses via the global `CacheStore`. Supports TTL and named groups:

```rust
#[get("/")]
#[intercept(Cache::ttl(30))]                         // cache for 30 seconds
async fn list(&self) -> Json<Vec<User>> { ... }

#[get("/")]
#[intercept(Cache::ttl(60).group("users"))]          // named cache group
async fn list(&self) -> Json<Vec<User>> { ... }
```

### CacheInvalidate

Clears a named cache group after method execution:

```rust
#[post("/")]
#[intercept(CacheInvalidate::group("users"))]
async fn create(&self, body: Json<CreateUser>) -> Result<Json<User>, HttpError> { ... }
```

## Combining interceptors

Multiple interceptors can be stacked on a single method. They wrap in declaration order (outermost first):

```rust
#[get("/")]
#[intercept(Logged::info())]
#[intercept(Timed::new())]
#[intercept(Cache::ttl(30).group("users"))]
async fn list(&self) -> Json<Vec<User>> { ... }
```

## License

Apache-2.0
