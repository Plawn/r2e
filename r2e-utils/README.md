# r2e-utils

Built-in interceptors for R2E — Logged, Timed, Cache, CacheInvalidate, Counted, and MetricTimed.

## Overview

Provides ready-to-use interceptors that implement the `Interceptor<R>` trait (generic over the return type `R`) from `r2e-core`. All calls are monomorphized at compile time for zero overhead.

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
#[intercept(Timed::threshold(100))]         // only logs if > 100ms
async fn slow(&self) -> Json<Data> { ... }
```

### Cache

Caches `Json<T>` responses via the `CacheStore` bean (`Arc<dyn CacheStore>`, resolved once at controller registration — a missing store is a compile error). Supports TTL and named groups:

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

### Counted

Increments a named counter on each invocation, logged via `tracing`:

```rust
#[get("/")]
#[intercept(Counted::new("user_list_total"))]
async fn list(&self) -> Json<Vec<User>> { ... }
```

### MetricTimed

Records the execution duration as a named metric, logged via `tracing`:

```rust
#[get("/")]
#[intercept(MetricTimed::new("user_list_duration"))]
async fn list(&self) -> Json<Vec<User>> { ... }
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
