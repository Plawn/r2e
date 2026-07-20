use std::future::Future;
use std::time::Duration;

use r2e_core::interceptors::{Interceptor, InterceptorContext};

/// Log level for `Logged` and `Timed` interceptors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Log a message at the given level using `tracing`.
pub fn log_at_level(level: LogLevel, method: &str, msg: &str) {
    match level {
        LogLevel::Trace => tracing::trace!(method = method, "{}", msg),
        LogLevel::Debug => tracing::debug!(method = method, "{}", msg),
        LogLevel::Info => tracing::info!(method = method, "{}", msg),
        LogLevel::Warn => tracing::warn!(method = method, "{}", msg),
        LogLevel::Error => tracing::error!(method = method, "{}", msg),
    }
}

// ---------------------------------------------------------------------------
// Logged
// ---------------------------------------------------------------------------

/// Logs entry and exit of a method at the specified level.
pub struct Logged {
    pub level: LogLevel,
}

impl Logged {
    pub fn new() -> Self {
        Logged {
            level: LogLevel::Info,
        }
    }
    pub fn info() -> Self {
        Logged {
            level: LogLevel::Info,
        }
    }
    pub fn debug() -> Self {
        Logged {
            level: LogLevel::Debug,
        }
    }
    pub fn warn() -> Self {
        Logged {
            level: LogLevel::Warn,
        }
    }
    pub fn trace() -> Self {
        Logged {
            level: LogLevel::Trace,
        }
    }
    pub fn error() -> Self {
        Logged {
            level: LogLevel::Error,
        }
    }
    pub fn level(level: LogLevel) -> Self {
        Logged { level }
    }
}

impl Default for Logged {
    fn default() -> Self {
        Self::new()
    }
}

impl r2e_core::SelfBuilt for Logged {}

impl<R: Send> Interceptor<R> for Logged {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let level = self.level;
        let method_name = ctx.method_name;
        async move {
            log_at_level(level, method_name, "entering");
            let result = next().await;
            log_at_level(level, method_name, "exiting");
            result
        }
    }
}

// ---------------------------------------------------------------------------
// Timed
// ---------------------------------------------------------------------------

/// Measures and logs the execution time of a method.
///
/// If `threshold_ms` is set, only logs when execution exceeds the threshold.
pub struct Timed {
    pub level: LogLevel,
    pub threshold_ms: Option<u64>,
}

impl Timed {
    pub fn new() -> Self {
        Timed {
            level: LogLevel::Info,
            threshold_ms: None,
        }
    }
    pub fn info() -> Self {
        Timed {
            level: LogLevel::Info,
            threshold_ms: None,
        }
    }
    pub fn debug() -> Self {
        Timed {
            level: LogLevel::Debug,
            threshold_ms: None,
        }
    }
    pub fn warn() -> Self {
        Timed {
            level: LogLevel::Warn,
            threshold_ms: None,
        }
    }
    pub fn threshold(ms: u64) -> Self {
        Timed {
            level: LogLevel::Info,
            threshold_ms: Some(ms),
        }
    }
    pub fn threshold_warn(ms: u64) -> Self {
        Timed {
            level: LogLevel::Warn,
            threshold_ms: Some(ms),
        }
    }
}

impl Default for Timed {
    fn default() -> Self {
        Self::new()
    }
}

impl r2e_core::SelfBuilt for Timed {}

impl<R: Send> Interceptor<R> for Timed {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let level = self.level;
        let threshold_ms = self.threshold_ms;
        let method_name = ctx.method_name;
        async move {
            let start = std::time::Instant::now();
            let result = next().await;
            let elapsed_ms = start.elapsed().as_millis() as u64;
            match threshold_ms {
                Some(threshold) if elapsed_ms <= threshold => {}
                _ => log_at_level(level, method_name, &format!("elapsed_ms={elapsed_ms}")),
            }
            result
        }
    }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

/// Caches the response of a method using the application's
/// [`CacheStore`](r2e_cache::CacheStore) bean.
///
/// `Cache` is the *spec*: the store (an `Arc<dyn CacheStore>` bean) is
/// declared in `Deps` — a missing store is a compile error at
/// `register_controller()` — and pulled once at wiring time into the built
/// [`CacheInterceptor`]. Provide one on the builder:
///
/// ```ignore
/// .provide(r2e_cache::InMemoryStore::shared())   // Arc<dyn CacheStore>
/// ```
///
/// Works with `r2e_core::http::Json<T>` where `T: Serialize + DeserializeOwned`.
///
/// # Usage
/// ```ignore
/// #[intercept(Cache::ttl(30))]
/// #[intercept(Cache::ttl(30).group("users"))]
/// #[intercept(Cache::with_key(30, format!("user:{}", id)))]
/// ```
pub struct Cache {
    ttl: Duration,
    key: Option<String>,
    group: Option<String>,
}

impl Cache {
    pub fn ttl(seconds: u64) -> Self {
        Cache {
            ttl: Duration::from_secs(seconds),
            key: None,
            group: None,
        }
    }

    pub fn with_key(seconds: u64, key: String) -> Self {
        Cache {
            ttl: Duration::from_secs(seconds),
            key: Some(key),
            group: None,
        }
    }

    pub fn group(mut self, group: &str) -> Self {
        self.group = Some(group.into());
        self
    }

    fn full_key(&self, controller_name: &str, method_name: &str) -> String {
        let prefix = self.group.as_deref().unwrap_or_else(|| "");
        let prefix = if prefix.is_empty() {
            format!("__{}_{}", controller_name, method_name)
        } else {
            prefix.to_string()
        };
        let suffix = self.key.as_deref().unwrap_or("default");
        format!("{}:{}", prefix, suffix)
    }
}

/// The built product of the [`Cache`] spec: holds the resolved store.
pub struct CacheInterceptor {
    store: std::sync::Arc<dyn r2e_cache::CacheStore>,
    config: Cache,
}

impl r2e_core::DecoratorSpec for Cache {
    type Product = CacheInterceptor;
    type Deps = r2e_core::type_list::TCons<
        std::sync::Arc<dyn r2e_cache::CacheStore>,
        r2e_core::type_list::TNil,
    >;

    fn build(self, ctx: &r2e_core::BeanContext) -> CacheInterceptor {
        CacheInterceptor {
            store: ctx.get(),
            config: self,
        }
    }
}

impl<R> Interceptor<R> for CacheInterceptor
where
    R: r2e_core::Cacheable,
{
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let store = self.store.clone();
        let key = self.config.full_key(ctx.controller_name, ctx.method_name);
        let ttl = self.config.ttl;
        async move {
            // Cache hit
            if let Some(cached) = store.get(&key).await {
                if let Some(val) = R::from_cache(&cached) {
                    return val;
                }
                // Deserialization failed — remove stale entry
                store.remove(&key).await;
            }
            // Cache miss
            let result = next().await;
            if let Some(bytes) = result.to_cache() {
                store.set(&key, bytes, ttl).await;
            }
            result
        }
    }
}

// ---------------------------------------------------------------------------
// CacheInvalidate
// ---------------------------------------------------------------------------

/// Invalidates all cache entries in a named group after the wrapped method executes.
///
/// Like [`Cache`], this is a spec: the store bean is resolved once at wiring
/// time into the built [`CacheInvalidateInterceptor`].
///
/// # Usage
/// ```ignore
/// #[intercept(CacheInvalidate::group("users"))]
/// ```
pub struct CacheInvalidate {
    group: String,
}

impl CacheInvalidate {
    pub fn group(name: &str) -> Self {
        CacheInvalidate { group: name.into() }
    }
}

/// The built product of the [`CacheInvalidate`] spec.
pub struct CacheInvalidateInterceptor {
    store: std::sync::Arc<dyn r2e_cache::CacheStore>,
    group: String,
}

impl r2e_core::DecoratorSpec for CacheInvalidate {
    type Product = CacheInvalidateInterceptor;
    type Deps = r2e_core::type_list::TCons<
        std::sync::Arc<dyn r2e_cache::CacheStore>,
        r2e_core::type_list::TNil,
    >;

    fn build(self, ctx: &r2e_core::BeanContext) -> CacheInvalidateInterceptor {
        CacheInvalidateInterceptor {
            store: ctx.get(),
            group: self.group,
        }
    }
}

impl<R: Send> Interceptor<R> for CacheInvalidateInterceptor {
    fn around<F, Fut>(&self, _ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let store = self.store.clone();
        let group = self.group.clone();
        async move {
            let result = next().await;
            store.remove_by_prefix(&format!("{}:", group)).await;
            result
        }
    }
}

// ---------------------------------------------------------------------------
// Counted
// ---------------------------------------------------------------------------

/// Increments a named counter on each invocation, logged via `tracing`.
///
/// # Usage
/// ```ignore
/// #[intercept(Counted::new("user_list_total"))]
/// async fn list(&self) -> Json<Vec<User>> { ... }
/// ```
pub struct Counted {
    pub metric_name: String,
    pub level: LogLevel,
}

impl Counted {
    /// Create a counter with the given metric name.
    pub fn new(name: &str) -> Self {
        Self {
            metric_name: name.to_string(),
            level: LogLevel::Info,
        }
    }

    /// Set the log level for the counter event.
    pub fn with_level(mut self, level: LogLevel) -> Self {
        self.level = level;
        self
    }
}

impl r2e_core::SelfBuilt for Counted {}

impl<R: Send> Interceptor<R> for Counted {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let metric_name = self.metric_name.clone();
        let level = self.level;
        let method_name = ctx.method_name;
        async move {
            let result = next().await;
            log_at_level(level, method_name, &format!("counted metric={metric_name}"));
            result
        }
    }
}

// ---------------------------------------------------------------------------
// MetricTimed
// ---------------------------------------------------------------------------

/// Records the execution duration as a named metric, logged via `tracing`.
///
/// # Usage
/// ```ignore
/// #[intercept(MetricTimed::new("user_list_duration"))]
/// async fn list(&self) -> Json<Vec<User>> { ... }
/// ```
pub struct MetricTimed {
    pub metric_name: String,
    pub level: LogLevel,
}

impl MetricTimed {
    /// Create a duration metric with the given name.
    pub fn new(name: &str) -> Self {
        Self {
            metric_name: name.to_string(),
            level: LogLevel::Info,
        }
    }

    /// Set the log level for the duration event.
    pub fn with_level(mut self, level: LogLevel) -> Self {
        self.level = level;
        self
    }
}

impl r2e_core::SelfBuilt for MetricTimed {}

impl<R: Send> Interceptor<R> for MetricTimed {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let metric_name = self.metric_name.clone();
        let level = self.level;
        let method_name = ctx.method_name;
        async move {
            let start = std::time::Instant::now();
            let result = next().await;
            let elapsed_ms = start.elapsed().as_millis() as u64;
            log_at_level(
                level,
                method_name,
                &format!("metric={metric_name} elapsed_ms={elapsed_ms}"),
            );
            result
        }
    }
}
