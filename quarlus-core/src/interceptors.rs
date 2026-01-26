use std::future::Future;

/// Context passed to each interceptor. `Copy` so it can be captured
/// by nested `async move` closures without ownership issues.
#[derive(Clone, Copy)]
pub struct InterceptorContext {
    pub method_name: &'static str,
    pub controller_name: &'static str,
}

/// Generic interceptor trait with an `around` pattern.
///
/// Each interceptor wraps the next computation. Interceptors are composed
/// by nesting: the outermost interceptor calls `next()` which runs the
/// next interceptor, and so on.
///
/// Type parameter `R` is the return type of the wrapped computation.
/// Interceptors that don't constrain the return type (e.g. `Logged`, `Timed`)
/// are generic over all `R: Send`. Interceptors that need specific capabilities
/// (e.g. `Cached` needs `Serialize + DeserializeOwned`) constrain `R` accordingly.
pub trait Interceptor<R> {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send;
}

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

/// Logs entry and exit of a method at the specified level.
pub struct Logged {
    pub level: LogLevel,
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

/// Measures and logs the execution time of a method.
///
/// If `threshold_ms` is set, only logs when execution exceeds the threshold.
pub struct Timed {
    pub level: LogLevel,
    pub threshold_ms: Option<u64>,
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

/// Caches the response of a method using a TTL cache.
///
/// Works with `axum::Json<T>` where `T: Serialize + DeserializeOwned`.
/// The cache stores serialized JSON strings keyed by the provided key.
pub struct Cached {
    pub cache: crate::TtlCache<String, String>,
    pub key: String,
}

impl<T> Interceptor<axum::Json<T>> for Cached
where
    T: serde::Serialize + serde::de::DeserializeOwned + Send,
{
    fn around<F, Fut>(
        &self,
        _ctx: InterceptorContext,
        next: F,
    ) -> impl Future<Output = axum::Json<T>> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = axum::Json<T>> + Send,
    {
        let cache = self.cache.clone();
        let key = self.key.clone();
        async move {
            // Cache hit
            if let Some(cached_str) = cache.get(&key) {
                if let Ok(val) = serde_json::from_str::<T>(&cached_str) {
                    return axum::Json(val);
                }
                // Deserialization failed — remove stale entry
                cache.remove(&key);
            }
            // Cache miss
            let result = next().await;
            if let Ok(serialized) = serde_json::to_string(&result.0) {
                cache.insert(key, serialized);
            }
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_logged_interceptor() {
        let logged = Logged {
            level: LogLevel::Info,
        };
        let ctx = InterceptorContext {
            method_name: "test_method",
            controller_name: "TestController",
        };
        let result = logged.around(ctx, || async { 42 }).await;
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn test_timed_interceptor() {
        let timed = Timed {
            level: LogLevel::Info,
            threshold_ms: None,
        };
        let ctx = InterceptorContext {
            method_name: "test_method",
            controller_name: "TestController",
        };
        let result = timed.around(ctx, || async { "hello" }).await;
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn test_timed_with_threshold() {
        let timed = Timed {
            level: LogLevel::Warn,
            threshold_ms: Some(1000),
        };
        let ctx = InterceptorContext {
            method_name: "fast_method",
            controller_name: "TestController",
        };
        // Fast call should not log (threshold not exceeded)
        let result = timed.around(ctx, || async { 99 }).await;
        assert_eq!(result, 99);
    }

    #[tokio::test]
    async fn test_nested_interceptors() {
        let logged = Logged {
            level: LogLevel::Debug,
        };
        let timed = Timed {
            level: LogLevel::Info,
            threshold_ms: None,
        };
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
    async fn test_cached_interceptor() {
        use crate::TtlCache;
        use std::time::Duration;

        let cache = TtlCache::new(Duration::from_secs(60));
        let ctx = InterceptorContext {
            method_name: "cached_method",
            controller_name: "TestController",
        };

        // First call — cache miss
        let cached = Cached {
            cache: cache.clone(),
            key: "test:default".to_string(),
        };
        let result: axum::Json<Vec<String>> = cached
            .around(ctx, || async {
                axum::Json(vec!["a".to_string(), "b".to_string()])
            })
            .await;
        assert_eq!(result.0, vec!["a".to_string(), "b".to_string()]);

        // Second call — cache hit (should return same data)
        let cached2 = Cached {
            cache: cache.clone(),
            key: "test:default".to_string(),
        };
        let result2: axum::Json<Vec<String>> = cached2
            .around(ctx, || async {
                // This should NOT be called because of cache hit
                axum::Json(vec!["c".to_string()])
            })
            .await;
        assert_eq!(result2.0, vec!["a".to_string(), "b".to_string()]);
    }
}
