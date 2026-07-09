use r2e_core::beans::BeanRegistry;
use r2e_core::guards::{
    Guard, GuardContext, Identity, PathParams, PreAuthGuard, PreAuthGuardContext,
};
use r2e_core::http::{HeaderMap, Uri};
use r2e_core::DecoratorSpec;
use r2e_rate_limit::{
    PreAuthRateLimitGuard, PreRateLimit, RateLimit, RateLimitGuard, RateLimitKeyKind,
    RateLimitRegistry,
};

struct TestIdentity {
    sub: String,
}

impl Identity for TestIdentity {
    fn sub(&self) -> &str {
        &self.sub
    }
}

async fn build_user_guard(max: u64, window_secs: u64) -> RateLimitGuard {
    let mut registry = BeanRegistry::new();
    registry.provide(RateLimitRegistry::default());
    let ctx = registry.resolve().await.expect("graph must resolve");
    <RateLimit as DecoratorSpec>::build(RateLimit::per_user(max, window_secs), &ctx)
}

async fn build_pre_guard(config: PreRateLimit) -> PreAuthRateLimitGuard {
    let mut registry = BeanRegistry::new();
    registry.provide(RateLimitRegistry::default());
    let ctx = registry.resolve().await.expect("graph must resolve");
    <PreRateLimit as DecoratorSpec>::build(config, &ctx)
}

fn guard_ctx<'a>(
    headers: &'a HeaderMap,
    uri: &'a Uri,
    identity: Option<&'a TestIdentity>,
) -> GuardContext<'a, TestIdentity> {
    GuardContext {
        method_name: "list",
        controller_name: "TestController",
        headers,
        uri,
        path_params: PathParams::EMPTY,
        identity,
    }
}

#[r2e_core::test]
async fn per_user_guard_builds_with_registry() {
    let guard = build_user_guard(2, 60).await;
    assert_eq!(guard.key, RateLimitKeyKind::User);
    assert_eq!(guard.max, 2);
    assert_eq!(guard.window_secs, 60);
}

#[r2e_core::test]
async fn per_user_guard_allows_then_blocks() {
    let guard = build_user_guard(2, 60).await;
    let headers = HeaderMap::new();
    let uri: Uri = "/api/things".parse().unwrap();
    let identity = TestIdentity {
        sub: "alice".to_string(),
    };
    let ctx = guard_ctx(&headers, &uri, Some(&identity));

    assert!(guard.check(&ctx).await.is_ok());
    assert!(guard.check(&ctx).await.is_ok());
    assert!(guard.check(&ctx).await.is_err());
}

#[r2e_core::test]
async fn per_user_guard_keys_per_subject() {
    let guard = build_user_guard(1, 60).await;
    let headers = HeaderMap::new();
    let uri: Uri = "/api/things".parse().unwrap();

    let alice = TestIdentity {
        sub: "alice".to_string(),
    };
    let bob = TestIdentity {
        sub: "bob".to_string(),
    };

    let alice_ctx = guard_ctx(&headers, &uri, Some(&alice));
    let bob_ctx = guard_ctx(&headers, &uri, Some(&bob));

    assert!(guard.check(&alice_ctx).await.is_ok());
    assert!(guard.check(&alice_ctx).await.is_err());
    // Different subject has an independent bucket.
    assert!(guard.check(&bob_ctx).await.is_ok());
}

#[r2e_core::test]
async fn pre_global_guard_shares_one_bucket() {
    let guard = build_pre_guard(PreRateLimit::global(2, 60)).await;
    assert_eq!(guard.key, RateLimitKeyKind::Global);

    let headers = HeaderMap::new();
    let uri: Uri = "/api/things".parse().unwrap();
    let ctx = PreAuthGuardContext {
        method_name: "list",
        controller_name: "TestController",
        headers: &headers,
        uri: &uri,
        path_params: PathParams::EMPTY,
    };

    assert!(guard.check(&ctx).await.is_ok());
    assert!(guard.check(&ctx).await.is_ok());
    assert!(guard.check(&ctx).await.is_err());
}

#[r2e_core::test]
async fn pre_ip_guard_keys_per_ip() {
    let guard = build_pre_guard(PreRateLimit::per_ip(1, 60)).await;
    assert_eq!(guard.key, RateLimitKeyKind::Ip);

    let uri: Uri = "/api/things".parse().unwrap();

    let mut headers_a = HeaderMap::new();
    headers_a.insert("x-forwarded-for", "1.1.1.1".parse().unwrap());
    let ctx_a = PreAuthGuardContext {
        method_name: "list",
        controller_name: "TestController",
        headers: &headers_a,
        uri: &uri,
        path_params: PathParams::EMPTY,
    };

    let mut headers_b = HeaderMap::new();
    headers_b.insert("x-forwarded-for", "2.2.2.2".parse().unwrap());
    let ctx_b = PreAuthGuardContext {
        method_name: "list",
        controller_name: "TestController",
        headers: &headers_b,
        uri: &uri,
        path_params: PathParams::EMPTY,
    };

    assert!(guard.check(&ctx_a).await.is_ok());
    assert!(guard.check(&ctx_a).await.is_err());
    // Different IP has an independent bucket.
    assert!(guard.check(&ctx_b).await.is_ok());
}
