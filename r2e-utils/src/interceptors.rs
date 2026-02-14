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
        Logged { level: LogLevel::Info }
    }
    pub fn info() -> Self {
        Logged { level: LogLevel::Info }
    }
    pub fn debug() -> Self {
        Logged { level: LogLevel::Debug }
    }
    pub fn warn() -> Self {
        Logged { level: LogLevel::Warn }
    }
    pub fn trace() -> Self {
        Logged { level: LogLevel::Trace }
    }
    pub fn error() -> Self {
        Logged { level: LogLevel::Error }
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

impl<R: Send, S: Send + Sync> Interceptor<R, S> for Logged {
    fn around<F, Fut>(&self, ctx: InterceptorContext<'_, S>, next: F) -> impl Future<Output = R> + Send
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
        Timed { level: LogLevel::Info, threshold_ms: None }
    }
    pub fn info() -> Self {
        Timed { level: LogLevel::Info, threshold_ms: None }
    }
    pub fn debug() -> Self {
        Timed { level: LogLevel::Debug, threshold_ms: None }
    }
    pub fn warn() -> Self {
        Timed { level: LogLevel::Warn, threshold_ms: None }
    }
    pub fn threshold(ms: u64) -> Self {
        Timed { level: LogLevel::Info, threshold_ms: Some(ms) }
    }
    pub fn threshold_warn(ms: u64) -> Self {
        Timed { level: LogLevel::Warn, threshold_ms: Some(ms) }
    }
}

impl Default for Timed {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: Send, S: Send + Sync> Interceptor<R, S> for Timed {
    fn around<F, Fut>(&self, ctx: InterceptorContext<'_, S>, next: F) -> impl Future<Output = R> + Send
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
                _ => log_at_level(
                    level,
                    method_name,
                    &format!("elapsed_ms={elapsed_ms}"),
                ),
            }
            result
        }
    }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

/// Caches the response of a method using the global [`CacheStore`](r2e_cache::CacheStore).
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
        let prefix = self.group.as_deref().unwrap_or_else(|| {
            ""
        });
        let prefix = if prefix.is_empty() {
            format!("__{}_{}", controller_name, method_name)
        } else {
            prefix.to_string()
        };
        let suffix = self.key.as_deref().unwrap_or("default");
        format!("{}:{}", prefix, suffix)
    }
}

impl<R, S: Send + Sync> Interceptor<R, S> for Cache
where
    R: r2e_core::Cacheable,
{
    fn around<F, Fut>(
        &self,
        ctx: InterceptorContext<'_, S>,
        next: F,
    ) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let store = r2e_cache::cache_backend();
        let key = self.full_key(ctx.controller_name, ctx.method_name);
        let ttl = self.ttl;
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
/// # Usage
/// ```ignore
/// #[intercept(CacheInvalidate::group("users"))]
/// ```
pub struct CacheInvalidate {
    group: String,
}

impl CacheInvalidate {
    pub fn group(name: &str) -> Self {
        CacheInvalidate {
            group: name.into(),
        }
    }
}

impl<R: Send, S: Send + Sync> Interceptor<R, S> for CacheInvalidate {
    fn around<F, Fut>(&self, _ctx: InterceptorContext<'_, S>, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let group = self.group.clone();
        async move {
            let result = next().await;
            r2e_cache::cache_backend()
                .remove_by_prefix(&format!("{}:", group))
                .await;
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

impl<R: Send, S: Send + Sync> Interceptor<R, S> for Counted {
    fn around<F, Fut>(&self, ctx: InterceptorContext<'_, S>, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let metric_name = self.metric_name.clone();
        let level = self.level;
        let method_name = ctx.method_name;
        async move {
            let result = next().await;
            log_at_level(
                level,
                method_name,
                &format!("counted metric={metric_name}"),
            );
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

impl<R: Send, S: Send + Sync> Interceptor<R, S> for MetricTimed {
    fn around<F, Fut>(&self, ctx: InterceptorContext<'_, S>, next: F) -> impl Future<Output = R> + Send
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

#[cfg(test)]
mod tests {
    use super::*;

    // A dummy state for tests.
    #[derive(Clone)]
    struct TestState;

    fn test_ctx(state: &TestState) -> InterceptorContext<'_, TestState> {
        InterceptorContext {
            method_name: "test_method",
            controller_name: "TestController",
            state,
        }
    }

    #[tokio::test]
    async fn test_logged_interceptor() {
        let logged = Logged::info();
        let state = TestState;
        let result = logged.around(test_ctx(&state), || async { 42 }).await;
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn test_logged_constructors() {
        assert_eq!(Logged::new().level, LogLevel::Info);
        assert_eq!(Logged::info().level, LogLevel::Info);
        assert_eq!(Logged::debug().level, LogLevel::Debug);
        assert_eq!(Logged::warn().level, LogLevel::Warn);
        assert_eq!(Logged::trace().level, LogLevel::Trace);
        assert_eq!(Logged::error().level, LogLevel::Error);
        assert_eq!(Logged::level(LogLevel::Warn).level, LogLevel::Warn);
    }

    #[tokio::test]
    async fn test_timed_interceptor() {
        let timed = Timed::info();
        let state = TestState;
        let result = timed.around(test_ctx(&state), || async { "hello" }).await;
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn test_timed_with_threshold() {
        let timed = Timed::threshold_warn(1000);
        let state = TestState;
        let ctx = InterceptorContext {
            method_name: "fast_method",
            controller_name: "TestController",
            state: &state,
        };
        // Fast call should not log (threshold not exceeded)
        let result = timed.around(ctx, || async { 99 }).await;
        assert_eq!(result, 99);
    }

    #[tokio::test]
    async fn test_timed_constructors() {
        assert_eq!(Timed::new().level, LogLevel::Info);
        assert!(Timed::new().threshold_ms.is_none());
        assert_eq!(Timed::info().level, LogLevel::Info);
        assert_eq!(Timed::debug().level, LogLevel::Debug);
        assert_eq!(Timed::warn().level, LogLevel::Warn);
        assert_eq!(Timed::threshold(100).threshold_ms, Some(100));
        assert_eq!(Timed::threshold_warn(200).level, LogLevel::Warn);
        assert_eq!(Timed::threshold_warn(200).threshold_ms, Some(200));
    }

    #[tokio::test]
    async fn test_nested_interceptors() {
        let logged = Logged::debug();
        let timed = Timed::info();
        let state = TestState;
        let state_ref: &_ = &state;

        let result = logged
            .around(test_ctx(state_ref), move || async move {
                timed
                    .around(test_ctx(state_ref), || async move { "nested_result" })
                    .await
            })
            .await;
        assert_eq!(result, "nested_result");
    }

    #[tokio::test]
    async fn test_cache_interceptor() {
        let state = TestState;
        let ctx = InterceptorContext {
            method_name: "cached_method",
            controller_name: "TestController",
            state: &state,
        };

        let cache = Cache::ttl(60);
        // First call — cache miss
        let result: r2e_core::http::Json<Vec<String>> = cache
            .around(ctx, || async {
                r2e_core::http::Json(vec!["a".to_string(), "b".to_string()])
            })
            .await;
        assert_eq!(result.0, vec!["a".to_string(), "b".to_string()]);

        // Second call — cache hit (same key)
        let cache2 = Cache::ttl(60);
        let ctx2 = InterceptorContext {
            method_name: "cached_method",
            controller_name: "TestController",
            state: &state,
        };
        let result2: r2e_core::http::Json<Vec<String>> = cache2
            .around(ctx2, || async {
                // Should NOT be called because of cache hit
                r2e_core::http::Json(vec!["c".to_string()])
            })
            .await;
        assert_eq!(result2.0, vec!["a".to_string(), "b".to_string()]);
    }

    #[tokio::test]
    async fn test_cache_invalidate_interceptor() {
        let state = TestState;
        let ctx = InterceptorContext {
            method_name: "create",
            controller_name: "TestController",
            state: &state,
        };

        // Pre-populate cache under group prefix
        let store = r2e_cache::cache_backend();
        store
            .set("mygroup:item1", bytes::Bytes::from("\"val\""), std::time::Duration::from_secs(60))
            .await;

        let invalidator = CacheInvalidate::group("mygroup");
        let result = invalidator
            .around(ctx, || async { 42 })
            .await;
        assert_eq!(result, 42);

        // Entry should be gone
        assert_eq!(store.get("mygroup:item1").await, None);
    }
}
