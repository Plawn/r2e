use r2e_core::interceptors::{Interceptor, InterceptorContext};
use r2e_utils::{Cache, CacheInvalidate, Logged, LogLevel, Timed};

fn test_ctx() -> InterceptorContext {
    InterceptorContext {
        method_name: "test_method",
        controller_name: "TestController",
    }
}

#[r2e_core::test]
async fn test_logged_interceptor() {
    let logged = Logged::info();
    let result = logged.around(test_ctx(), || async { 42 }).await;
    assert_eq!(result, 42);
}

#[r2e_core::test]
async fn test_logged_constructors() {
    assert_eq!(Logged::new().level, LogLevel::Info);
    assert_eq!(Logged::info().level, LogLevel::Info);
    assert_eq!(Logged::debug().level, LogLevel::Debug);
    assert_eq!(Logged::warn().level, LogLevel::Warn);
    assert_eq!(Logged::trace().level, LogLevel::Trace);
    assert_eq!(Logged::error().level, LogLevel::Error);
    assert_eq!(Logged::level(LogLevel::Warn).level, LogLevel::Warn);
}

#[r2e_core::test]
async fn test_timed_interceptor() {
    let timed = Timed::info();
    let result = timed.around(test_ctx(), || async { "hello" }).await;
    assert_eq!(result, "hello");
}

#[r2e_core::test]
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

#[r2e_core::test]
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

#[r2e_core::test]
async fn test_nested_interceptors() {
    let logged = Logged::debug();
    let timed = Timed::info();

    let result = logged
        .around(test_ctx(), move || async move {
            timed
                .around(test_ctx(), || async move { "nested_result" })
                .await
        })
        .await;
    assert_eq!(result, "nested_result");
}

#[r2e_core::test]
async fn test_cache_interceptor() {
    let ctx = InterceptorContext {
        method_name: "cached_method",
        controller_name: "TestController",
    };

    let cache = Cache::ttl(60);
    // First call -- cache miss
    let result: r2e_core::http::Json<Vec<String>> = cache
        .around(ctx, || async {
            r2e_core::http::Json(vec!["a".to_string(), "b".to_string()])
        })
        .await;
    assert_eq!(result.0, vec!["a".to_string(), "b".to_string()]);

    // Second call -- cache hit (same key)
    let cache2 = Cache::ttl(60);
    let ctx2 = InterceptorContext {
        method_name: "cached_method",
        controller_name: "TestController",
    };
    let result2: r2e_core::http::Json<Vec<String>> = cache2
        .around(ctx2, || async {
            // Should NOT be called because of cache hit
            r2e_core::http::Json(vec!["c".to_string()])
        })
        .await;
    assert_eq!(result2.0, vec!["a".to_string(), "b".to_string()]);
}

#[r2e_core::test]
async fn test_cache_invalidate_interceptor() {
    let ctx = InterceptorContext {
        method_name: "create",
        controller_name: "TestController",
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
