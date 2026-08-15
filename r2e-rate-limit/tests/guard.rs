use std::net::SocketAddr;

use r2e_core::beans::BeanRegistry;
use r2e_core::config::R2eConfig;
use r2e_core::guards::{
    Guard, GuardContext, Identity, PathParams, PreAuthGuard, PreAuthGuardContext,
};
use r2e_core::http::{HeaderMap, Uri};
use r2e_core::DecoratorSpec;
use r2e_rate_limit::{
    ConfiguredPreRateLimit, ConfiguredRateLimit, IpSource, PreAuthRateLimitGuard, PreRateLimit,
    RateLimit, RateLimitGuard, RateLimitKeyKind, RateLimitRegistry,
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

/// Build a config-resolved pre-auth guard from a YAML snippet.
async fn build_configured_pre_guard(
    yaml: &str,
    spec: ConfiguredPreRateLimit,
) -> PreAuthRateLimitGuard {
    let mut registry = BeanRegistry::new();
    registry.provide(RateLimitRegistry::default());
    registry.provide(R2eConfig::from_yaml_str(yaml).expect("valid yaml"));
    let ctx = registry.resolve().await.expect("graph must resolve");
    <ConfiguredPreRateLimit as DecoratorSpec>::build(spec, &ctx)
}

/// Build a config-resolved post-auth guard from a YAML snippet.
async fn build_configured_user_guard(yaml: &str, spec: ConfiguredRateLimit) -> RateLimitGuard {
    let mut registry = BeanRegistry::new();
    registry.provide(RateLimitRegistry::default());
    registry.provide(R2eConfig::from_yaml_str(yaml).expect("valid yaml"));
    let ctx = registry.resolve().await.expect("graph must resolve");
    <ConfiguredRateLimit as DecoratorSpec>::build(spec, &ctx)
}

fn guard_ctx<'a>(
    headers: &'a HeaderMap,
    uri: &'a Uri,
    identity: Option<&'a TestIdentity>,
) -> GuardContext<'a, TestIdentity> {
    named_guard_ctx("TestController", "list", headers, uri, identity)
}

fn named_guard_ctx<'a>(
    controller_name: &'static str,
    method_name: &'static str,
    headers: &'a HeaderMap,
    uri: &'a Uri,
    identity: Option<&'a TestIdentity>,
) -> GuardContext<'a, TestIdentity> {
    GuardContext {
        method_name,
        controller_name,
        method: r2e_core::default_method(),
        extensions: r2e_core::no_extensions(),
        headers,
        uri,
        peer_addr: None,
        path_params: PathParams::EMPTY,
        identity,
    }
}

fn pre_ctx<'a>(
    controller_name: &'static str,
    method_name: &'static str,
    headers: &'a HeaderMap,
    uri: &'a Uri,
    peer_addr: Option<SocketAddr>,
) -> PreAuthGuardContext<'a> {
    PreAuthGuardContext {
        method_name,
        controller_name,
        headers,
        uri,
        peer_addr,
        path_params: PathParams::EMPTY,
    }
}

fn xff(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", value.parse().unwrap());
    headers
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
        peer_addr: None,
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
        peer_addr: None,
        path_params: PathParams::EMPTY,
    };

    let mut headers_b = HeaderMap::new();
    headers_b.insert("x-forwarded-for", "2.2.2.2".parse().unwrap());
    let ctx_b = PreAuthGuardContext {
        method_name: "list",
        controller_name: "TestController",
        headers: &headers_b,
        uri: &uri,
        peer_addr: None,
        path_params: PathParams::EMPTY,
    };

    assert!(guard.check(&ctx_a).await.is_ok());
    assert!(guard.check(&ctx_a).await.is_err());
    // Different IP has an independent bucket.
    assert!(guard.check(&ctx_b).await.is_ok());
}

// ── Bucket keys are scoped by controller ────────────────────────────────────

#[r2e_core::test]
async fn per_user_guard_keys_per_controller() {
    let guard = build_user_guard(1, 60).await;
    let headers = HeaderMap::new();
    let uri: Uri = "/start".parse().unwrap();
    let identity = TestIdentity {
        sub: "alice".to_string(),
    };

    // Two controllers exposing a homonymous handler must not share a bucket.
    let respond = named_guard_ctx("RespondController", "start", &headers, &uri, Some(&identity));
    let preview = named_guard_ctx("PreviewController", "start", &headers, &uri, Some(&identity));

    assert!(guard.check(&respond).await.is_ok());
    assert!(guard.check(&respond).await.is_err());
    assert!(
        guard.check(&preview).await.is_ok(),
        "a different controller with the same handler name owns its own bucket"
    );
}

#[r2e_core::test]
async fn pre_guard_keys_per_controller() {
    let guard = build_pre_guard(PreRateLimit::global(1, 60)).await;
    let headers = HeaderMap::new();
    let uri: Uri = "/start".parse().unwrap();

    let respond = pre_ctx("RespondController", "start", &headers, &uri, None);
    let preview = pre_ctx("PreviewController", "start", &headers, &uri, None);

    assert!(guard.check(&respond).await.is_ok());
    assert!(guard.check(&respond).await.is_err());
    assert!(
        guard.check(&preview).await.is_ok(),
        "a different controller with the same handler name owns its own bucket"
    );
}

#[r2e_core::test]
async fn pre_guard_keys_per_method_within_a_controller() {
    let guard = build_pre_guard(PreRateLimit::global(1, 60)).await;
    let headers = HeaderMap::new();
    let uri: Uri = "/x".parse().unwrap();

    let start = pre_ctx("RespondController", "start", &headers, &uri, None);
    let answer = pre_ctx("RespondController", "answer", &headers, &uri, None);

    assert!(guard.check(&start).await.is_ok());
    assert!(guard.check(&start).await.is_err());
    assert!(guard.check(&answer).await.is_ok());
}

// ── Client-IP resolution ────────────────────────────────────────────────────

#[r2e_core::test]
async fn pre_ip_guard_falls_back_to_peer_address() {
    let guard = build_pre_guard(PreRateLimit::per_ip(1, 60)).await;
    assert_eq!(guard.ip_source, IpSource::ForwardedThenPeer);

    let headers = HeaderMap::new();
    let uri: Uri = "/x".parse().unwrap();
    let peer_a: SocketAddr = "10.0.0.1:5555".parse().unwrap();
    let peer_b: SocketAddr = "10.0.0.2:5555".parse().unwrap();

    let ctx_a = pre_ctx("C", "list", &headers, &uri, Some(peer_a));
    // Same host, different ephemeral port — must land in the same bucket.
    let peer_a2: SocketAddr = "10.0.0.1:6666".parse().unwrap();
    let ctx_a2 = pre_ctx("C", "list", &headers, &uri, Some(peer_a2));
    let ctx_b = pre_ctx("C", "list", &headers, &uri, Some(peer_b));

    assert!(guard.check(&ctx_a).await.is_ok());
    assert!(
        guard.check(&ctx_a2).await.is_err(),
        "the bucket keys on the IP, not the source port"
    );
    assert!(guard.check(&ctx_b).await.is_ok());
}

#[r2e_core::test]
async fn pre_ip_guard_prefers_forwarded_for_over_peer() {
    let guard = build_pre_guard(PreRateLimit::per_ip(1, 60)).await;
    let uri: Uri = "/x".parse().unwrap();
    let peer: SocketAddr = "10.0.0.1:5555".parse().unwrap();

    let headers_a = xff("1.1.1.1, 10.0.0.1");
    let headers_b = xff("2.2.2.2, 10.0.0.1");

    // Same proxy peer, different clients → independent buckets.
    let ctx_a = pre_ctx("C", "list", &headers_a, &uri, Some(peer));
    let ctx_b = pre_ctx("C", "list", &headers_b, &uri, Some(peer));

    assert!(guard.check(&ctx_a).await.is_ok());
    assert!(guard.check(&ctx_a).await.is_err());
    assert!(guard.check(&ctx_b).await.is_ok());
}

#[r2e_core::test]
async fn peer_ip_only_ignores_forwarded_for() {
    let guard = build_pre_guard(PreRateLimit::per_ip(1, 60).peer_ip_only()).await;
    assert_eq!(guard.ip_source, IpSource::Peer);

    let uri: Uri = "/x".parse().unwrap();
    let peer: SocketAddr = "10.0.0.1:5555".parse().unwrap();

    // A client forging distinct X-Forwarded-For values cannot escape its bucket.
    let headers_a = xff("1.1.1.1");
    let headers_b = xff("2.2.2.2");
    let ctx_a = pre_ctx("C", "list", &headers_a, &uri, Some(peer));
    let ctx_b = pre_ctx("C", "list", &headers_b, &uri, Some(peer));

    assert!(guard.check(&ctx_a).await.is_ok());
    assert!(guard.check(&ctx_b).await.is_err());
}

#[r2e_core::test]
async fn ip_guard_without_any_source_shares_the_unknown_bucket() {
    // No X-Forwarded-For and no peer address: everything falls into `unknown`
    // (and a one-time warning is logged).
    let guard = build_pre_guard(PreRateLimit::per_ip(1, 60)).await;
    let headers = HeaderMap::new();
    let uri: Uri = "/x".parse().unwrap();
    let ctx = pre_ctx("C", "list", &headers, &uri, None);

    assert!(guard.check(&ctx).await.is_ok());
    assert!(guard.check(&ctx).await.is_err());
}

#[r2e_core::test]
async fn empty_forwarded_for_falls_back_to_peer() {
    let guard = build_pre_guard(PreRateLimit::per_ip(1, 60)).await;
    let uri: Uri = "/x".parse().unwrap();
    let headers = xff("");
    let peer_a: SocketAddr = "10.0.0.1:1".parse().unwrap();
    let peer_b: SocketAddr = "10.0.0.2:1".parse().unwrap();

    let ctx_a = pre_ctx("C", "list", &headers, &uri, Some(peer_a));
    let ctx_b = pre_ctx("C", "list", &headers, &uri, Some(peer_b));

    assert!(guard.check(&ctx_a).await.is_ok());
    assert!(guard.check(&ctx_a).await.is_err());
    assert!(guard.check(&ctx_b).await.is_ok());
}

#[r2e_core::test]
async fn malformed_forwarded_for_falls_back_to_peer() {
    // An unparseable X-Forwarded-For must not mint a bucket of its own, and
    // must not suppress the peer fallback either.
    let guard = build_pre_guard(PreRateLimit::per_ip(1, 60)).await;
    let uri: Uri = "/x".parse().unwrap();
    let peer: SocketAddr = "10.0.0.1:1".parse().unwrap();

    let junk_a = xff("not-an-ip");
    let junk_b = xff("evil; DROP TABLE users");
    let ctx_a = pre_ctx("C", "list", &junk_a, &uri, Some(peer));
    let ctx_b = pre_ctx("C", "list", &junk_b, &uri, Some(peer));

    assert!(guard.check(&ctx_a).await.is_ok());
    assert!(
        guard.check(&ctx_b).await.is_err(),
        "two different junk header values from the same peer share one bucket"
    );
}

#[r2e_core::test]
async fn forwarded_for_with_a_port_shares_the_bare_address_bucket() {
    let guard = build_pre_guard(PreRateLimit::per_ip(1, 60)).await;
    let uri: Uri = "/x".parse().unwrap();

    let bare = xff("1.2.3.4");
    let ported = xff("1.2.3.4:9999");

    assert!(guard
        .check(&pre_ctx("C", "list", &bare, &uri, None))
        .await
        .is_ok());
    assert!(
        guard
            .check(&pre_ctx("C", "list", &ported, &uri, None))
            .await
            .is_err(),
        "the port is not part of the identity"
    );
}

#[r2e_core::test]
async fn ipv6_aliases_share_one_bucket() {
    let guard = build_pre_guard(PreRateLimit::per_ip(1, 60)).await;
    let uri: Uri = "/x".parse().unwrap();

    let long = xff("0:0:0:0:0:0:0:1");
    let short = xff("::1");
    let bracketed = xff("[::1]:8080");

    assert!(guard
        .check(&pre_ctx("C", "list", &long, &uri, None))
        .await
        .is_ok());
    assert!(guard
        .check(&pre_ctx("C", "list", &short, &uri, None))
        .await
        .is_err());
    assert!(guard
        .check(&pre_ctx("C", "list", &bracketed, &uri, None))
        .await
        .is_err());
}

// ── Per-user limits require an identity ─────────────────────────────────────

#[r2e_core::test]
async fn per_user_spec_declares_the_identity_requirement() {
    assert!(<RateLimit as DecoratorSpec>::REQUIRES_IDENTITY);
    assert!(<ConfiguredRateLimit as DecoratorSpec>::REQUIRES_IDENTITY);
    // Pre-auth limits (global / per-IP) run before authentication.
    assert!(!<PreRateLimit as DecoratorSpec>::REQUIRES_IDENTITY);
    assert!(!<ConfiguredPreRateLimit as DecoratorSpec>::REQUIRES_IDENTITY);
}

#[r2e_core::test]
async fn per_user_guard_rejects_a_request_without_identity() {
    let guard = build_user_guard(1, 60).await;
    let headers = HeaderMap::new();
    let uri: Uri = "/x".parse().unwrap();
    let anon = guard_ctx(&headers, &uri, None);

    // Fail closed: never a shared `anonymous` bucket.
    let err = guard
        .check(&anon)
        .await
        .expect_err("a per-user limit without an identity must be rejected");
    assert_eq!(err.status(), r2e_core::http::StatusCode::UNAUTHORIZED);

    // Repeat calls keep 401-ing (they are not consuming any bucket).
    let err = guard.check(&anon).await.expect_err("still rejected");
    assert_eq!(err.status(), r2e_core::http::StatusCode::UNAUTHORIZED);

    // And an authenticated caller still has their full budget.
    let identity = TestIdentity {
        sub: "alice".to_string(),
    };
    let ctx = guard_ctx(&headers, &uri, Some(&identity));
    assert!(guard.check(&ctx).await.is_ok());
    assert!(guard.check(&ctx).await.is_err());
}

// ── Zero windows are rejected at the construction site ──────────────────────

#[test]
#[should_panic(expected = "`window_secs` must be greater than 0")]
fn per_user_rejects_a_zero_window() {
    let _ = RateLimit::per_user(5, 0);
}

#[test]
#[should_panic(expected = "`window_secs` must be greater than 0")]
fn pre_global_rejects_a_zero_window() {
    let _ = PreRateLimit::global(5, 0);
}

#[test]
#[should_panic(expected = "`window_secs` must be greater than 0")]
fn pre_per_ip_rejects_a_zero_window() {
    let _ = PreRateLimit::per_ip(5, 0);
}

#[test]
#[should_panic(expected = "`window_secs` must be greater than 0")]
fn configured_pre_defaults_reject_a_zero_window() {
    let _ = ConfiguredPreRateLimit::global("rate-limit.public").defaults(5, 0);
}

#[test]
#[should_panic(expected = "`window_secs` must be greater than 0")]
fn configured_user_defaults_reject_a_zero_window() {
    let _ = ConfiguredRateLimit::per_user("rate-limit.api").defaults(5, 0);
}

// ── Config-resolved budgets ─────────────────────────────────────────────────

#[r2e_core::test]
async fn configured_pre_guard_reads_budget_from_config() {
    let guard = build_configured_pre_guard(
        "rate-limit:\n  public:\n    max: 3\n    window-secs: 120\n",
        ConfiguredPreRateLimit::per_ip("rate-limit.public").defaults(60, 60),
    )
    .await;

    assert_eq!(guard.max, 3);
    assert_eq!(guard.window_secs, 120);
    assert!(guard.enabled);
    assert_eq!(guard.key, RateLimitKeyKind::Ip);
}

#[r2e_core::test]
async fn configured_pre_guard_falls_back_to_defaults() {
    let guard = build_configured_pre_guard(
        "app:\n  name: test\n",
        ConfiguredPreRateLimit::global("rate-limit.public").defaults(7, 30),
    )
    .await;

    assert_eq!(guard.max, 7);
    assert_eq!(guard.window_secs, 30);
    assert_eq!(guard.key, RateLimitKeyKind::Global);
}

#[r2e_core::test]
async fn configured_pre_guard_can_be_disabled() {
    let guard = build_configured_pre_guard(
        "rate-limit:\n  public:\n    max: 1\n    enabled: false\n",
        ConfiguredPreRateLimit::global("rate-limit.public").defaults(1, 60),
    )
    .await;

    let headers = HeaderMap::new();
    let uri: Uri = "/x".parse().unwrap();
    let ctx = pre_ctx("C", "list", &headers, &uri, None);

    assert!(!guard.enabled);
    for _ in 0..5 {
        assert!(guard.check(&ctx).await.is_ok(), "disabled = always allowed");
    }
}

#[r2e_core::test]
async fn configured_pre_guard_can_distrust_forwarded_for() {
    let guard = build_configured_pre_guard(
        "rate-limit:\n  public:\n    trust-forwarded-for: false\n",
        ConfiguredPreRateLimit::per_ip("rate-limit.public").defaults(1, 60),
    )
    .await;

    assert_eq!(guard.ip_source, IpSource::Peer);

    let uri: Uri = "/x".parse().unwrap();
    let peer: SocketAddr = "10.0.0.1:1".parse().unwrap();
    let headers_a = xff("1.1.1.1");
    let headers_b = xff("2.2.2.2");

    assert!(guard
        .check(&pre_ctx("C", "list", &headers_a, &uri, Some(peer)))
        .await
        .is_ok());
    assert!(guard
        .check(&pre_ctx("C", "list", &headers_b, &uri, Some(peer)))
        .await
        .is_err());
}

#[r2e_core::test]
async fn configured_user_guard_reads_budget_from_config() {
    let guard = build_configured_user_guard(
        "rate-limit:\n  api:\n    max: 2\n    window-secs: 45\n",
        ConfiguredRateLimit::per_user("rate-limit.api").defaults(60, 60),
    )
    .await;

    assert_eq!(guard.max, 2);
    assert_eq!(guard.window_secs, 45);
    assert_eq!(guard.key, RateLimitKeyKind::User);

    let headers = HeaderMap::new();
    let uri: Uri = "/x".parse().unwrap();
    let identity = TestIdentity {
        sub: "alice".to_string(),
    };
    let ctx = guard_ctx(&headers, &uri, Some(&identity));

    assert!(guard.check(&ctx).await.is_ok());
    assert!(guard.check(&ctx).await.is_ok());
    assert!(guard.check(&ctx).await.is_err());
}

// ── Malformed configuration fails startup, never silently defaults ──────────

#[r2e_core::test]
#[should_panic(expected = "invalid configuration for `rate-limit.public.max`")]
async fn configured_pre_guard_rejects_a_non_numeric_max() {
    let _ = build_configured_pre_guard(
        "rate-limit:\n  public:\n    max: not-a-number\n",
        ConfiguredPreRateLimit::per_ip("rate-limit.public").defaults(60, 60),
    )
    .await;
}

#[r2e_core::test]
#[should_panic(expected = "invalid configuration for `rate-limit.public.window-secs`")]
async fn configured_pre_guard_rejects_a_non_numeric_window() {
    let _ = build_configured_pre_guard(
        "rate-limit:\n  public:\n    window-secs: sixty\n",
        ConfiguredPreRateLimit::per_ip("rate-limit.public").defaults(60, 60),
    )
    .await;
}

#[r2e_core::test]
#[should_panic(expected = "invalid configuration for `rate-limit.public.enabled`")]
async fn configured_pre_guard_rejects_a_non_boolean_enabled() {
    let _ = build_configured_pre_guard(
        "rate-limit:\n  public:\n    enabled: yes-please\n",
        ConfiguredPreRateLimit::per_ip("rate-limit.public").defaults(60, 60),
    )
    .await;
}

#[r2e_core::test]
#[should_panic(expected = "invalid configuration for `rate-limit.public.trust-forwarded-for`")]
async fn configured_pre_guard_rejects_a_non_boolean_trust_flag() {
    let _ = build_configured_pre_guard(
        "rate-limit:\n  public:\n    trust-forwarded-for: sometimes\n",
        ConfiguredPreRateLimit::per_ip("rate-limit.public").defaults(60, 60),
    )
    .await;
}

#[r2e_core::test]
#[should_panic(expected = "`rate-limit.public.window-secs` must be greater than 0")]
async fn configured_pre_guard_rejects_a_zero_window_from_config() {
    let _ = build_configured_pre_guard(
        "rate-limit:\n  public:\n    window-secs: 0\n",
        ConfiguredPreRateLimit::per_ip("rate-limit.public").defaults(60, 60),
    )
    .await;
}

#[r2e_core::test]
#[should_panic(expected = "invalid configuration for `rate-limit.api.max`")]
async fn configured_user_guard_rejects_a_non_numeric_max() {
    let _ = build_configured_user_guard(
        "rate-limit:\n  api:\n    max: plenty\n",
        ConfiguredRateLimit::per_user("rate-limit.api").defaults(60, 60),
    )
    .await;
}

#[r2e_core::test]
#[should_panic(expected = "`rate-limit.api.window-secs` must be greater than 0")]
async fn configured_user_guard_rejects_a_zero_window_from_config() {
    let _ = build_configured_user_guard(
        "rate-limit:\n  api:\n    window-secs: 0\n",
        ConfiguredRateLimit::per_user("rate-limit.api").defaults(60, 60),
    )
    .await;
}
