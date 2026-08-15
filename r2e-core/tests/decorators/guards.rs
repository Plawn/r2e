use std::net::{IpAddr, SocketAddr};

use r2e_core::guards::{
    parse_forwarded_ip, ClientIp, GuardContext, GuardError, Identity, NoIdentity, PathParams,
    PreAuthGuardContext,
};
use r2e_core::http::{HeaderMap, StatusCode, Uri};

struct TestIdentity {
    sub: String,
    email: Option<String>,
    claims: Option<serde_json::Value>,
}

impl TestIdentity {
    fn new(sub: &str) -> Self {
        Self {
            sub: sub.to_string(),
            email: None,
            claims: None,
        }
    }

    fn with_email(mut self, email: &str) -> Self {
        self.email = Some(email.to_string());
        self
    }
}

impl Identity for TestIdentity {
    fn sub(&self) -> &str {
        &self.sub
    }
    fn email(&self) -> Option<&str> {
        self.email.as_deref()
    }
    fn claims(&self) -> Option<&serde_json::Value> {
        self.claims.as_ref()
    }
}

fn make_uri(s: &str) -> Uri {
    s.parse().unwrap()
}

fn make_ctx<'a, I: Identity>(
    identity: Option<&'a I>,
    uri: &'a Uri,
    headers: &'a HeaderMap,
    path_params: PathParams<'a>,
) -> GuardContext<'a, I> {
    GuardContext {
        method_name: "test_method",
        controller_name: "TestController",
        method: r2e_core::default_method(),
        extensions: r2e_core::no_extensions(),
        headers,
        uri,
        peer_addr: None,
        path_params,
        identity,
    }
}

fn make_pre_auth_ctx<'a>(
    uri: &'a Uri,
    headers: &'a HeaderMap,
    path_params: PathParams<'a>,
) -> PreAuthGuardContext<'a> {
    PreAuthGuardContext {
        method_name: "test_method",
        controller_name: "TestController",
        headers,
        uri,
        peer_addr: None,
        path_params,
    }
}

// PathParams tests
#[test]
fn path_params_get_existing() {
    let pairs = [("id", "123")];
    let params = PathParams::from_pairs(&pairs);
    assert_eq!(params.get("id"), Some("123"));
}

#[test]
fn path_params_get_missing() {
    let pairs = [("id", "123")];
    let params = PathParams::from_pairs(&pairs);
    assert_eq!(params.get("other"), None);
}

#[test]
fn path_params_empty() {
    assert_eq!(PathParams::EMPTY.get("anything"), None);
}

#[test]
fn path_params_parse_existing() {
    let pairs = [("id", "123")];
    let params = PathParams::from_pairs(&pairs);
    let parsed: u64 = params.parse("id").unwrap();
    assert_eq!(parsed, 123);
}

#[test]
fn path_params_parse_missing_returns_internal_error() {
    let err: GuardError = PathParams::EMPTY.parse::<u64>("id").unwrap_err();
    assert_eq!(err.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(err.message.contains("missing path parameter `id`"));
}

#[test]
fn path_params_parse_invalid_returns_bad_request() {
    let pairs = [("id", "abc")];
    let params = PathParams::from_pairs(&pairs);
    let err = params.parse::<u64>("id").unwrap_err();
    assert_eq!(err.status, StatusCode::BAD_REQUEST);
    assert!(err.message.contains("invalid path parameter `id`"));
}

// NoIdentity tests
#[test]
fn no_identity_sub_is_empty() {
    assert_eq!(NoIdentity.sub(), "");
}

// GuardContext accessor tests
#[test]
fn guard_context_identity_sub() {
    let id = TestIdentity::new("user-1");
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx = make_ctx(Some(&id), &uri, &headers, PathParams::EMPTY);
    assert_eq!(ctx.identity_sub(), Some("user-1"));
}

#[test]
fn guard_context_identity_email() {
    let id = TestIdentity::new("user-1").with_email("a@b.com");
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx = make_ctx(Some(&id), &uri, &headers, PathParams::EMPTY);
    assert_eq!(ctx.identity_email(), Some("a@b.com"));
}

#[test]
fn guard_context_identity_none() {
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx: GuardContext<'_, TestIdentity> = make_ctx(None, &uri, &headers, PathParams::EMPTY);
    assert_eq!(ctx.identity_sub(), None);
    assert_eq!(ctx.identity_email(), None);
}

#[test]
fn guard_context_path() {
    let uri = make_uri("/users?q=1");
    let headers = HeaderMap::new();
    let ctx: GuardContext<'_, NoIdentity> = make_ctx(None, &uri, &headers, PathParams::EMPTY);
    assert_eq!(ctx.path(), "/users");
}

#[test]
fn guard_context_query_string() {
    let uri = make_uri("/users?q=1");
    let headers = HeaderMap::new();
    let ctx: GuardContext<'_, NoIdentity> = make_ctx(None, &uri, &headers, PathParams::EMPTY);
    assert_eq!(ctx.query_string(), Some("q=1"));
}

#[test]
fn guard_context_path_param() {
    let pairs = [("id", "42")];
    let uri = make_uri("/users/42");
    let headers = HeaderMap::new();
    let ctx: GuardContext<'_, NoIdentity> =
        make_ctx(None, &uri, &headers, PathParams::from_pairs(&pairs));
    assert_eq!(ctx.path_param("id"), Some("42"));
    assert_eq!(ctx.path_param("missing"), None);
}

#[test]
fn guard_context_parse_path_param() {
    let pairs = [("id", "42")];
    let uri = make_uri("/users/42");
    let headers = HeaderMap::new();
    let ctx: GuardContext<'_, NoIdentity> =
        make_ctx(None, &uri, &headers, PathParams::from_pairs(&pairs));
    let parsed: u64 = ctx.parse_path_param("id").unwrap();
    assert_eq!(parsed, 42);
}

#[test]
fn pre_auth_guard_context_parse_path_param() {
    let pairs = [("id", "42")];
    let uri = make_uri("/users/42");
    let headers = HeaderMap::new();
    let ctx = make_pre_auth_ctx(&uri, &headers, PathParams::from_pairs(&pairs));
    let parsed: u64 = ctx.parse_path_param("id").unwrap();
    assert_eq!(parsed, 42);
}

#[test]
fn path_param_descriptor_exposes_name() {
    let param = r2e_core::PathParam::<u64>::new("id");
    assert_eq!(param.name(), "id");
    assert_eq!(param.as_ref(), "id");
}

#[test]
fn guard_context_method_name() {
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx: GuardContext<'_, NoIdentity> = make_ctx(None, &uri, &headers, PathParams::EMPTY);
    assert_eq!(ctx.method_name, "test_method");
}

#[test]
fn guard_context_controller_name() {
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx: GuardContext<'_, NoIdentity> = make_ctx(None, &uri, &headers, PathParams::EMPTY);
    assert_eq!(ctx.controller_name, "TestController");
}

// ── Client IP resolution (X-Forwarded-For parsing + peer fallback) ──────────

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn xff_headers(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", value.parse().unwrap());
    headers
}

fn ctx_with<'a>(
    uri: &'a Uri,
    headers: &'a HeaderMap,
    peer_addr: Option<SocketAddr>,
) -> PreAuthGuardContext<'a> {
    PreAuthGuardContext {
        method_name: "test_method",
        controller_name: "TestController",
        headers,
        uri,
        peer_addr,
        path_params: PathParams::EMPTY,
    }
}

#[test]
fn parse_forwarded_ip_accepts_bare_addresses() {
    assert_eq!(parse_forwarded_ip("1.2.3.4"), Some(ip("1.2.3.4")));
    assert_eq!(parse_forwarded_ip("2001:db8::1"), Some(ip("2001:db8::1")));
    assert_eq!(parse_forwarded_ip("  1.2.3.4  "), Some(ip("1.2.3.4")));
}

#[test]
fn parse_forwarded_ip_strips_ports() {
    assert_eq!(parse_forwarded_ip("1.2.3.4:5678"), Some(ip("1.2.3.4")));
    assert_eq!(parse_forwarded_ip("[::1]:8080"), Some(ip("::1")));
}

#[test]
fn parse_forwarded_ip_accepts_bracketed_ipv6_without_port() {
    assert_eq!(parse_forwarded_ip("[2001:db8::1]"), Some(ip("2001:db8::1")));
}

#[test]
fn parse_forwarded_ip_canonicalizes_ipv6_aliases() {
    // Two spellings of the same address must render to one bucket key.
    let long = parse_forwarded_ip("0:0:0:0:0:0:0:1").unwrap();
    let short = parse_forwarded_ip("::1").unwrap();
    assert_eq!(long, short);
    assert_eq!(long.to_string(), "::1");
}

#[test]
fn parse_forwarded_ip_rejects_non_addresses() {
    for bad in [
        "",
        "   ",
        "unknown",
        "not-an-ip",
        "evil'; DROP TABLE users;--",
        "999.999.999.999",
        "1.2.3.4.5",
        "[::1",
        "1.2.3.4:notaport",
    ] {
        assert_eq!(parse_forwarded_ip(bad), None, "must reject {bad:?}");
    }
}

#[test]
fn forwarded_ip_takes_the_leftmost_entry() {
    let headers = xff_headers("1.2.3.4, 10.0.0.1, 10.0.0.2");
    let uri = make_uri("/x");
    let ctx = ctx_with(&uri, &headers, None);
    assert_eq!(ctx.forwarded_ip(), Some(ip("1.2.3.4")));
    // The raw accessor still exposes the unvalidated string.
    assert_eq!(ctx.forwarded_for(), Some("1.2.3.4"));
}

#[test]
fn malformed_forwarded_for_falls_back_to_the_peer() {
    let headers = xff_headers("garbage, 1.2.3.4");
    let uri = make_uri("/x");
    let peer: SocketAddr = "10.0.0.7:4444".parse().unwrap();
    let ctx = ctx_with(&uri, &headers, Some(peer));

    // Never repaired from a later (equally untrusted) entry.
    assert_eq!(ctx.forwarded_ip(), None);
    assert_eq!(ctx.client_ip(), Some(ClientIp::Peer(ip("10.0.0.7"))));
    assert_eq!(ctx.client_ip().unwrap().to_string(), "10.0.0.7");
}

#[test]
fn malformed_forwarded_for_without_peer_resolves_to_nothing() {
    let headers = xff_headers("not-an-ip");
    let uri = make_uri("/x");
    let ctx = ctx_with(&uri, &headers, None);
    assert_eq!(ctx.client_ip(), None);
}

#[test]
fn client_ip_prefers_a_parseable_forwarded_entry() {
    let headers = xff_headers("[::1]:9999");
    let uri = make_uri("/x");
    let peer: SocketAddr = "10.0.0.7:4444".parse().unwrap();
    let ctx = ctx_with(&uri, &headers, Some(peer));
    assert_eq!(ctx.client_ip(), Some(ClientIp::Forwarded(ip("::1"))));
    assert_eq!(ctx.client_ip().unwrap().ip(), ip("::1"));
}

#[test]
fn peer_ip_drops_the_source_port() {
    let headers = HeaderMap::new();
    let uri = make_uri("/x");
    let ctx = ctx_with(&uri, &headers, Some("10.0.0.7:4444".parse().unwrap()));
    assert_eq!(ctx.peer_ip(), Some(ip("10.0.0.7")));
}

#[test]
fn guard_context_resolves_the_client_ip_too() {
    // Same resolution on the post-auth context.
    let headers = xff_headers("bogus");
    let uri = make_uri("/x");
    let peer: SocketAddr = "192.0.2.9:1234".parse().unwrap();
    let ctx: GuardContext<'_, NoIdentity> = GuardContext {
        method_name: "test_method",
        controller_name: "TestController",
        method: r2e_core::default_method(),
        extensions: r2e_core::no_extensions(),
        headers: &headers,
        uri: &uri,
        peer_addr: Some(peer),
        path_params: PathParams::EMPTY,
        identity: None,
    };
    assert_eq!(ctx.forwarded_ip(), None);
    assert_eq!(ctx.client_ip(), Some(ClientIp::Peer(ip("192.0.2.9"))));
}

#[test]
fn guard_context_identity_claims() {
    let claims = serde_json::json!({"aud": "test-app", "scope": "read"});
    let mut id = TestIdentity::new("user-1");
    id.claims = Some(claims.clone());
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx = make_ctx(Some(&id), &uri, &headers, PathParams::EMPTY);
    assert_eq!(ctx.identity_claims(), Some(&claims));
}
