# Caching

R2E offers caching at two levels:

- **Response caching**: the `Cache` and `CacheInvalidate` interceptors
  (from `r2e-utils`) store handler results in a pluggable `CacheStore` bean.
- **In-process caching**: `TtlCache<K, V>` (from `r2e-cache`) is a thread-safe
  map with expiry that you use directly in your own beans.

There is no global cache. The store is an ordinary bean, so a test or another
deployment can swap it.

## Setup

```toml
r2e = { version = "0.15", features = ["cache"] }
```

The interceptors come with the default `utils` feature and are in
`r2e::prelude`. The `cache` feature exposes `r2e::r2e_cache`, where
`InMemoryStore`, `CacheStore` and `TtlCache` live. Both are part of `full`.

Provide a store on the builder:

```rust
AppBuilder::new()
    .provide(r2e::r2e_cache::InMemoryStore::shared()) // Arc<dyn CacheStore>
    // ...
```

`Cache` and `CacheInvalidate` declare the store as a dependency. If no
`Arc<dyn CacheStore>` bean is provided, `register_controller` fails to
compile.

## Caching responses

```rust
#[routes]
impl UserController {
    #[get("/cached")]
    #[intercept(Cache::ttl(30).group("users"))]
    async fn cached_list(&self) -> Json<Vec<User>> {
        Json(self.user_service.list().await)
    }

    #[post("/")]
    #[intercept(CacheInvalidate::group("users"))]
    async fn create(&self, Json(body): Json<CreateUserRequest>) -> Json<User> {
        Json(self.user_service.create(body.name, body.email).await)
    }
}
```

| Spec | Effect |
|---|---|
| `Cache::ttl(secs)` | Cache the result for `secs` seconds |
| `.group("name")` | Put the entry in a named group, so it can be invalidated together with others |
| `Cache::with_key(secs, key)` | Use a fixed key instead of the generated one |
| `CacheInvalidate::group("name")` | After the method runs, remove every entry of the group |

**Keys do not include the request arguments.** The interceptor is built once,
at registration. Without a group, the key is made from the controller and
method name, plus the `with_key` value if one is given. With a group, the key
is `<group>:<key>`. Use response caching for routes whose result does not depend
on path, query or identity, such as a public list or reference data. For
per-argument caching, inject the store or a `TtlCache` and cache inside the
method.

### What can be cached

`Cache` needs the return type to implement `Cacheable`:

| Type | Notes |
|---|---|
| `Json<T>` | `T: Serialize + DeserializeOwned + Send` |
| `Result<T, E>` | `T: Cacheable`. Only `Ok` values are cached, errors pass through. |
| Your own type | `#[derive(Serialize, Deserialize, Cacheable)]` |

### Combining with guards

Guards run before interceptors, so `#[roles]` or `#[guard]` together with
`Cache` works: an unauthorized caller is rejected before the cache is read.

```rust
#[get("/admin/users")]
#[roles("admin")]
#[intercept(Cache::ttl(30).group("admin_users"))]
async fn admin_list(&self) -> Json<Vec<User>> { /* ... */ }
```

`Cache` does **not** work with a `#[managed]` parameter on the same handler.
Read the store bean (`#[inject] store: Arc<dyn CacheStore>`) and cache by hand
there.

## The `CacheStore` trait

`InMemoryStore` is the built-in store. To back the cache with Redis or another
shared store, implement `CacheStore` and provide your type as
`Arc<dyn CacheStore>`:

| Method | Purpose |
|---|---|
| `get(key)` | Return the cached bytes, if any |
| `set(key, value: Bytes, ttl: Duration)` | Store bytes with an expiry |
| `remove(key)` | Delete one entry |
| `clear()` | Delete everything |
| `remove_by_prefix(prefix)` | Delete a group (used by `CacheInvalidate`) |

The methods return boxed futures, so the trait is object safe.

## `TtlCache` in your own beans

```rust
use r2e::r2e_cache::TtlCache;
use std::time::Duration;

#[derive(Clone)]
pub struct ExchangeRates {
    cache: TtlCache<String, f64>,
}

impl ExchangeRates {
    pub fn new() -> Self {
        Self { cache: TtlCache::new(Duration::from_secs(300)) }
    }

    pub async fn rate(&self, currency: &str) -> f64 {
        if let Some(rate) = self.cache.get(&currency.to_string()) {
            return rate;
        }
        let rate = fetch_rate(currency).await;
        self.cache.insert(currency.to_string(), rate);
        rate
    }
}
```

`TtlCache` offers `new(ttl)`, `get`, `insert`, `remove`, `clear` and
`evict_expired` (call it from a scheduled task to free memory early). Expired
entries are never returned by `get`.

## Testing

Provide a fresh `InMemoryStore::shared()` per test boot (or override the bean
with `override_bean`) so tests never see entries from each other. To check
caching, call the route twice and verify the service ran once. Then call the
invalidating route and verify the next read reaches the service again.
`examples/example-app` uses both interceptors in
`src/controllers/user_controller.rs`.

## See also

- Interceptor internals: [docs/claude/guards-interceptors.md](https://github.com/plawn/r2e/blob/master/docs/claude/guards-interceptors.md)
- [Interceptors](./interceptors.md)
- [Rate Limiting](../security/rate-limiting.md)
