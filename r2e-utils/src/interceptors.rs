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

impl<R: Send> Interceptor<R> for Logged {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let level = self.level;
        async move {
            log_at_level(level, ctx.method_name, "entering");
            let result = next().await;
            log_at_level(level, ctx.method_name, "exiting");
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

impl<R: Send> Interceptor<R> for Timed {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let level = self.level;
        let threshold_ms = self.threshold_ms;
        async move {
            let start = std::time::Instant::now();
            let result = next().await;
            let elapsed_ms = start.elapsed().as_millis() as u64;
            match threshold_ms {
                Some(threshold) if elapsed_ms <= threshold => {}
                _ => log_at_level(
                    level,
                    ctx.method_name,
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

    fn full_key(&self, ctx: &InterceptorContext) -> String {
        let prefix = self.group.as_deref().unwrap_or_else(|| {
            ""
        });
        let prefix = if prefix.is_empty() {
            format!("__{}_{}", ctx.controller_name, ctx.method_name)
        } else {
            prefix.to_string()
        };
        let suffix = self.key.as_deref().unwrap_or("default");
        format!("{}:{}", prefix, suffix)
    }
}

impl<T> Interceptor<r2e_core::http::Json<T>> for Cache
where
    T: serde::Serialize + serde::de::DeserializeOwned + Send,
{
    fn around<F, Fut>(
        &self,
        ctx: InterceptorContext,
        next: F,
    ) -> impl Future<Output = r2e_core::http::Json<T>> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = r2e_core::http::Json<T>> + Send,
    {
        let store = r2e_cache::cache_backend();
        let key = self.full_key(&ctx);
        let ttl = self.ttl;
        async move {
            // Cache hit
            if let Some(cached) = store.get(&key).await {
                if let Ok(val) = serde_json::from_str::<T>(&cached) {
                    return r2e_core::http::Json(val);
                }
                // Deserialization failed — remove stale entry
                store.remove(&key).await;
            }
            // Cache miss
            let result = next().await;
            if let Ok(s) = serde_json::to_string(&result.0) {
                store.set(&key, s, ttl).await;
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

impl<R: Send> Interceptor<R> for CacheInvalidate {
    fn around<F, Fut>(&self, _ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_logged_interceptor() {
        let logged = Logged::info();
        let ctx = InterceptorContext {
            method_name: "test_method",
            controller_name: "TestController",
        };
        let result = logged.around(ctx, || async { 42 }).await;
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
        let ctx = InterceptorContext {
            method_name: "test_method",
            controller_name: "TestController",
        };
        let result = timed.around(ctx, || async { "hello" }).await;
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn test_timed_with_threshold() {
        let timed = Timed::threshold_warn(1000);
        let ctx = InterceptorContext {
            method_name: "fast_method",
            controller_name: "TestController",
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
        let ctx = InterceptorContext {
            method_name: "nested",
            controller_name: "TestController",
        };

        let result = logged
            .around(ctx, || async move {
                timed
                    .around(ctx, || async move { "nested_result" })
                    .await
            })
            .await;
        assert_eq!(result, "nested_result");
    }

    #[tokio::test]
    async fn test_cache_interceptor() {
        let ctx = InterceptorContext {
            method_name: "cached_method",
            controller_name: "TestController",
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
        let result2: r2e_core::http::Json<Vec<String>> = cache2
            .around(ctx, || async {
                // Should NOT be called because of cache hit
                r2e_core::http::Json(vec!["c".to_string()])
            })
            .await;
        assert_eq!(result2.0, vec!["a".to_string(), "b".to_string()]);
    }

    #[tokio::test]
    async fn test_cache_invalidate_interceptor() {
        let ctx = InterceptorContext {
            method_name: "create",
            controller_name: "TestController",
        };

        // Pre-populate cache under group prefix
        let store = r2e_cache::cache_backend();
        store
            .set("mygroup:item1", "\"val\"".to_string(), std::time::Duration::from_secs(60))
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
