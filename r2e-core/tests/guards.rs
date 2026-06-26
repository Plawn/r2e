use r2e_core::guards::{
    GuardContext, GuardError, Identity, NoIdentity, PathParams, PreAuthGuardContext,
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
        headers,
        uri,
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
