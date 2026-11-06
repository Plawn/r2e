//! `#[inject(identity)]` on MCP members without `mcp.auth`: the parameter is
//! extracted exactly like an HTTP route's identity — through
//! `FromRequestPartsVia` against the application state.
//!
//! * an app-defined identity (`AppUser`, a plain `FromRequestParts` reading
//!   an `x-user` header — bridged through `ViaAxum`) on a `#[tool]` and a
//!   `#[resource]`;
//! * `Option<AppUser>`: `None` when absent; a required one surfaces the
//!   extractor's own rejection message as a JSON-RPC error;
//! * `AuthenticatedUser` through its bean-backed extractor (`ViaBean`), with
//!   the `Arc<JwtClaimsValidator>` bean provided and a `TestJwt` bearer;
//! * the struct-level request scope (`#[inject(identity)]` /
//!   `#[inject(request)]` fields on the service struct): extracted once per
//!   HTTP request on the endpoint, failing the whole request closed, read by
//!   every member family and `#[request_helper]`, `#[roles]` gating and
//!   `tools/list` filtering on the identity's roles, session ↔ subject
//!   binding, and the caller probe's autoref priority.

use std::sync::Arc;

use r2e_core::http::extract::{FromRequestParts, OptionalFromRequestParts};
use r2e_core::http::header::Parts;
use r2e_core::http::{Body, HeaderMap, Request, Routes, StatusCode};
use r2e_core::prelude::*;
use r2e_core::{AppBuilder, Identity, Rejection, RejectionKind};
use r2e_mcp::AppBuilderMcpExt;
use r2e_mcp::McpServer;
use r2e_security::AuthenticatedUser;
use r2e_test::TestJwt;
use serde_json::{json, Value};

use crate::support;

// ── An app-defined identity ────────────────────────────────────────────────

const MISSING_USER: &str = "missing x-user header";

/// The application's own identity type: whatever the `x-user` header says.
#[derive(Clone)]
struct AppUser(String);

fn header_user(parts: &Parts) -> Option<AppUser> {
    parts
        .headers
        .get("x-user")
        .and_then(|value| value.to_str().ok())
        .map(|user| AppUser(user.to_owned()))
}

impl<S: Send + Sync> FromRequestParts<S> for AppUser {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        header_user(parts)
            .ok_or_else(|| Rejection::new(RejectionKind::Unauthenticated, MISSING_USER))
    }
}

impl<S: Send + Sync> OptionalFromRequestParts<S> for AppUser {
    type Rejection = Rejection;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        Ok(header_user(parts))
    }
}

#[controller]
struct AppIdentityTools;

#[routes]
impl AppIdentityTools {
    /// A required app identity on a tool.
    #[tool]
    async fn hello(&self, #[inject(identity)] user: AppUser) -> String {
        format!("hello {}", user.0)
    }

    /// An optional app identity on a tool.
    #[tool]
    async fn maybe_hello(&self, #[inject(identity)] user: Option<AppUser>) -> String {
        format!(
            "hello {}",
            user.map_or_else(|| "anonymous".to_string(), |u| u.0)
        )
    }

    /// A required app identity on a resource.
    #[resource(uri = "r2e://app/me")]
    async fn me(&self, #[inject(identity)] user: AppUser) -> String {
        format!("me:{}", user.0)
    }
}

async fn app_identity_app() -> Routes {
    AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<AppIdentityTools>()
        .build()
}

async fn call_as(router: &Routes, session: &str, user: Option<&str>, name: &str) -> Value {
    let headers: Vec<(&str, &str)> = user.map(|u| ("x-user", u)).into_iter().collect();
    support::tools_call_with_headers(router, "/mcp", session, &headers, name, json!({})).await
}

fn text(message: &Value) -> &str {
    message["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content in {message}"))
}

#[r2e_core::test]
async fn an_app_identity_reaches_a_tool() {
    let router = app_identity_app().await;
    let session = support::initialize(&router, "/mcp").await;

    let message = call_as(&router, &session, Some("alice"), "hello").await;
    assert_eq!(text(&message), "hello alice", "{message}");

    let message = call_as(&router, &session, Some("alice"), "maybe_hello").await;
    assert_eq!(text(&message), "hello alice", "{message}");
}

#[r2e_core::test]
async fn an_app_identity_reaches_a_resource() {
    let router = app_identity_app().await;
    let session = support::initialize(&router, "/mcp").await;

    let response = support::post_with_headers(
        &router,
        "/mcp",
        Some(&session),
        &[("x-user", "carol")],
        &json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "resources/read",
            "params": { "uri": "r2e://app/me" }
        }),
    )
    .await;
    let message = response.message();
    assert_eq!(
        message["result"]["contents"][0]["text"], "me:carol",
        "{message}"
    );

    // Without the header the resource read is rejected by the extractor.
    let message = support::resources_read(&router, "/mcp", &session, "r2e://app/me").await;
    assert_eq!(message["error"]["data"], "unauthorized", "{message}");
    assert_eq!(message["error"]["message"], MISSING_USER, "{message}");
}

#[r2e_core::test]
async fn an_absent_app_identity_is_none_or_the_extractors_rejection() {
    let router = app_identity_app().await;
    let session = support::initialize(&router, "/mcp").await;

    let message = call_as(&router, &session, None, "maybe_hello").await;
    assert_eq!(text(&message), "hello anonymous", "{message}");

    let message = call_as(&router, &session, None, "hello").await;
    assert_eq!(message["error"]["code"], -32600, "{message}");
    assert_eq!(message["error"]["data"], "unauthorized", "{message}");
    assert_eq!(message["error"]["message"], MISSING_USER, "{message}");
}

// ── AuthenticatedUser through the JWT bean ─────────────────────────────────

#[controller]
struct JwtIdentityTools;

#[routes]
impl JwtIdentityTools {
    /// `AuthenticatedUser` validated by the `JwtClaimsValidator` bean — the
    /// same extractor an HTTP route uses.
    #[tool]
    async fn whoami(&self, #[inject(identity)] user: AuthenticatedUser) -> String {
        format!("{}:{}", user.sub, user.roles.join(","))
    }

    #[tool]
    async fn maybe_whoami(&self, #[inject(identity)] user: Option<AuthenticatedUser>) -> String {
        user.map_or_else(|| "anonymous".to_string(), |u| u.sub)
    }
}

#[r2e_core::test]
async fn authenticated_user_is_validated_by_the_jwt_bean() {
    let jwt = TestJwt::new();
    let router = AppBuilder::new()
        .provide(Arc::new(jwt.claims_validator()))
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<JwtIdentityTools>()
        .build();
    let session = support::initialize(&router, "/mcp").await;

    let token = jwt.token_builder("alice").roles(&["admin"]).build();
    let bearer = format!("Bearer {token}");
    let auth = [("authorization", bearer.as_str())];

    let message =
        support::tools_call_with_headers(&router, "/mcp", &session, &auth, "whoami", json!({}))
            .await;
    assert_eq!(text(&message), "alice:admin", "{message}");

    let message = support::tools_call_with_headers(
        &router,
        "/mcp",
        &session,
        &auth,
        "maybe_whoami",
        json!({}),
    )
    .await;
    assert_eq!(text(&message), "alice", "{message}");

    // No bearer: optional → None, required → unauthorized.
    let message = support::tools_call(&router, "/mcp", &session, "maybe_whoami", json!({})).await;
    assert_eq!(text(&message), "anonymous", "{message}");
    let message = support::tools_call(&router, "/mcp", &session, "whoami", json!({})).await;
    assert_eq!(message["error"]["data"], "unauthorized", "{message}");

    // A forged bearer is rejected, not treated as anonymous.
    let forged = [("authorization", "Bearer not-a-jwt")];
    let message =
        support::tools_call_with_headers(&router, "/mcp", &session, &forged, "whoami", json!({}))
            .await;
    assert_eq!(message["error"]["data"], "unauthorized", "{message}");
}

// ── Ownership ──────────────────────────────────────────────────────────────

/// A bean whose only job is to be observed from outside the state.
struct Sentinel;

/// The identity closures hold the application state, and the state holds
/// `McpRegistry`, which validates against the catalog those closures live
/// in. The registry holds the catalog's key index, not the catalog, so
/// dropping the endpoint releases the state — otherwise every
/// `TestApp::boot` / dev-reload cycle would leak the whole bean graph.
#[r2e_core::test]
async fn dropping_the_endpoint_releases_the_state() {
    let sentinel = Arc::new(Sentinel);
    let observed = Arc::downgrade(&sentinel);
    // Stateless: a session would be owned by the transport's session task,
    // which only the serve-time shutdown token ends — not what is measured
    // here.
    let router = AppBuilder::new()
        .plugin(McpServer::new().stateless(true))
        .provide(sentinel)
        .build_state()
        .await
        .register_mcp_service::<AppIdentityTools>()
        .build();
    assert!(observed.upgrade().is_some(), "the state owns the sentinel");

    // One call through an identity member: the closure really ran against
    // the captured state.
    let response = support::post_with_headers(
        &router,
        "/mcp",
        None,
        &[("x-user", "alice")],
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "hello", "arguments": {} }
        }),
    )
    .await;
    assert_eq!(
        text(response.message()),
        "hello alice",
        "{}",
        response.raw_body
    );

    drop(router);
    // rmcp serves a stateless request on a spawned task that holds a clone of
    // the service until it finishes winding down, which can land on another
    // worker after the response body is read. Give it a bounded grace
    // period: a real cycle never releases, so the assertion keeps its teeth.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while observed.upgrade().is_some() && std::time::Instant::now() < deadline {
        r2e_core::rt::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        observed.upgrade().is_none(),
        "the state must be released with the endpoint (McpRegistry ↔ catalog cycle)"
    );
}

// ── Struct-level request scope (endpoint identity) ─────────────────────────
//
// The same `#[inject(identity)]` / `#[inject(request)]` fields an HTTP
// controller declares, on the MCP service struct: extracted ONCE per HTTP
// request by the endpoint layer, before any JSON-RPC message is dispatched,
// and read by every member as `self.<field>` on the request façade.

/// The app identity declared once on the struct: every request on the
/// endpoint — `initialize` included — must carry it.
#[controller]
struct StructAppTools {
    #[inject(identity)]
    user: AppUser,
    #[inject(request)]
    headers: HeaderMap,
}

#[routes]
impl StructAppTools {
    #[tool]
    async fn who(&self) -> String {
        format!("who {}", self.user.0)
    }

    /// Through a `#[request_helper]` reading both request-scoped fields.
    #[tool]
    async fn tag(&self) -> String {
        self.tagged("tag")
    }

    #[resource(uri = "r2e://struct/me")]
    async fn me(&self) -> String {
        format!("me:{}", self.user.0)
    }

    #[prompt]
    async fn greet(&self) -> String {
        format!("greet {}", self.user.0)
    }

    #[request_helper]
    fn tagged(&self, what: &str) -> String {
        let tag = self
            .headers
            .get("x-tag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-");
        format!("{what}:{}:{tag}", self.user.0)
    }
}

#[controller]
struct OptionalStructTools {
    #[inject(identity)]
    user: Option<AppUser>,
}

#[routes]
impl OptionalStructTools {
    #[tool]
    async fn maybe_who(&self) -> String {
        self.user
            .as_ref()
            .map_or_else(|| "anonymous".to_string(), |u| format!("who {}", u.0))
    }
}

async fn struct_app() -> Routes {
    AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<StructAppTools>()
        .build()
}

fn as_user(user: &str) -> [(&str, &str); 1] {
    [("x-user", user)]
}

#[r2e_core::test]
async fn a_struct_identity_rejects_the_whole_request_without_it() {
    let router = struct_app().await;

    // `initialize` itself fails closed, with the extractor's own rejection.
    let response =
        support::post_with_headers(&router, "/mcp", None, &[], &support::initialize_body()).await;
    assert_eq!(
        response.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        response.raw_body
    );
    assert!(
        !response.raw_body.contains("jsonrpc"),
        "no JSON-RPC message was dispatched: {}",
        response.raw_body
    );
    assert!(
        response.raw_body.contains(MISSING_USER),
        "{}",
        response.raw_body
    );

    // With it, the session opens — and every later request must carry it
    // too: a bare `tools/list` on that session is rejected the same way.
    let session = support::initialize_with_headers(&router, "/mcp", &as_user("alice")).await;
    let response = support::post(
        &router,
        "/mcp",
        Some(&session),
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        response.raw_body
    );
    assert!(
        !response.raw_body.contains("jsonrpc"),
        "{}",
        response.raw_body
    );
}

#[r2e_core::test]
async fn struct_fields_reach_every_member_family() {
    let router = struct_app().await;
    let session = support::initialize_with_headers(&router, "/mcp", &as_user("alice")).await;

    let message = support::tools_call_with_headers(
        &router,
        "/mcp",
        &session,
        &as_user("alice"),
        "who",
        json!({}),
    )
    .await;
    assert_eq!(text(&message), "who alice", "{message}");

    // `#[request_helper]` on the façade, reading the identity AND the
    // `#[inject(request)]` field.
    let headers = [("x-user", "alice"), ("x-tag", "blue")];
    let message =
        support::tools_call_with_headers(&router, "/mcp", &session, &headers, "tag", json!({}))
            .await;
    assert_eq!(text(&message), "tag:alice:blue", "{message}");

    let response = support::post_with_headers(
        &router,
        "/mcp",
        Some(&session),
        &as_user("alice"),
        &json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/read", "params": { "uri": "r2e://struct/me" } }),
    )
    .await;
    assert_eq!(
        response.result()["contents"][0]["text"],
        "me:alice",
        "{}",
        response.raw_body
    );

    let response = support::post_with_headers(
        &router,
        "/mcp",
        Some(&session),
        &as_user("alice"),
        &json!({ "jsonrpc": "2.0", "id": 5, "method": "prompts/get", "params": { "name": "greet" } }),
    )
    .await;
    assert_eq!(
        response.result()["messages"][0]["content"]["text"],
        "greet alice",
        "{}",
        response.raw_body
    );
}

#[r2e_core::test]
async fn an_optional_struct_identity_admits_anonymous_requests() {
    let router = AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<OptionalStructTools>()
        .build();
    let session = support::initialize(&router, "/mcp").await;

    let message = support::tools_call(&router, "/mcp", &session, "maybe_who", json!({})).await;
    assert_eq!(text(&message), "anonymous", "{message}");

    let message = support::tools_call_with_headers(
        &router,
        "/mcp",
        &session,
        &as_user("bob"),
        "maybe_who",
        json!({}),
    )
    .await;
    assert_eq!(text(&message), "who bob", "{message}");
}

/// The endpoint is shared: every registered service's request scope runs on
/// every request, so one required struct identity fails the endpoint closed
/// for the members of the other services too (like one HTTP controller's
/// identity does not leak onto another's routes — but here the layer is the
/// endpoint, not the route).
#[r2e_core::test]
async fn every_registered_scope_must_pass_on_the_shared_endpoint() {
    let router = AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<StructAppTools>()
        .register_mcp_service::<AppIdentityTools>()
        .build();

    let response =
        support::post_with_headers(&router, "/mcp", None, &[], &support::initialize_body()).await;
    assert_eq!(
        response.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        response.raw_body
    );

    let session = support::initialize_with_headers(&router, "/mcp", &as_user("alice")).await;
    let message = support::tools_call_with_headers(
        &router,
        "/mcp",
        &session,
        &as_user("alice"),
        "maybe_hello",
        json!({}),
    )
    .await;
    assert_eq!(text(&message), "hello alice", "{message}");
    let message = support::tools_call_with_headers(
        &router,
        "/mcp",
        &session,
        &as_user("alice"),
        "who",
        json!({}),
    )
    .await;
    assert_eq!(text(&message), "who alice", "{message}");
}

/// CORS preflight carries no credentials by design: the request scope is
/// skipped for `OPTIONS`.
#[r2e_core::test]
async fn preflight_skips_the_request_scope() {
    use tower::ServiceExt;
    let router = struct_app().await;
    let request = Request::builder()
        .method("OPTIONS")
        .uri("/mcp")
        .header("host", "localhost")
        .header("origin", "http://localhost:5173")
        .header("access-control-request-method", "POST")
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
}

// ── Struct-level identity with roles: filtering + session binding ──────────

#[controller]
struct SecuredStructTools {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl SecuredStructTools {
    #[tool]
    async fn whoami(&self) -> String {
        self.user.sub.clone()
    }

    /// Gated by the struct identity's roles (enforced by the guard, and
    /// filtered out of `tools/list` for callers without the role).
    #[tool]
    #[roles("admin")]
    async fn admin_only(&self) -> String {
        format!("admin:{}", self.user.sub)
    }
}

async fn secured_struct_app(jwt: &TestJwt) -> Routes {
    AppBuilder::new()
        .provide(Arc::new(jwt.claims_validator()))
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<SecuredStructTools>()
        .build()
}

fn bearer(jwt: &TestJwt, sub: &str, roles: &[&str]) -> String {
    format!("Bearer {}", jwt.token_builder(sub).roles(roles).build())
}

async fn list_as(router: &Routes, session: &str, bearer: &str) -> Vec<String> {
    let response = support::post_with_headers(
        router,
        "/mcp",
        Some(session),
        &[("authorization", bearer)],
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    support::tool_names(response.result())
}

#[r2e_core::test]
async fn roles_on_a_struct_identity_gate_the_member_and_filter_the_list() {
    let jwt = TestJwt::new();
    let router = secured_struct_app(&jwt).await;

    let alice = bearer(&jwt, "alice", &[]);
    let session =
        support::initialize_with_headers(&router, "/mcp", &[("authorization", &alice)]).await;
    let tools = list_as(&router, &session, &alice).await;
    assert_eq!(tools, vec!["whoami".to_string()], "{tools:?}");
    let message = support::tools_call_with_headers(
        &router,
        "/mcp",
        &session,
        &[("authorization", &alice)],
        "whoami",
        json!({}),
    )
    .await;
    assert_eq!(text(&message), "alice", "{message}");
    let message = support::tools_call_with_headers(
        &router,
        "/mcp",
        &session,
        &[("authorization", &alice)],
        "admin_only",
        json!({}),
    )
    .await;
    assert_eq!(message["error"]["data"], "forbidden", "{message}");

    let root = bearer(&jwt, "root", &["admin"]);
    let session =
        support::initialize_with_headers(&router, "/mcp", &[("authorization", &root)]).await;
    let tools = list_as(&router, &session, &root).await;
    assert_eq!(
        tools,
        vec!["admin_only".to_string(), "whoami".to_string()],
        "{tools:?}"
    );
    let message = support::tools_call_with_headers(
        &router,
        "/mcp",
        &session,
        &[("authorization", &root)],
        "admin_only",
        json!({}),
    )
    .await;
    assert_eq!(text(&message), "admin:root", "{message}");
}

/// A session opened by one subject is a 404 for another — rmcp's own
/// "unknown session" answer, so the second caller learns nothing — exactly
/// what the `mcp.auth` layer does for its principal.
#[r2e_core::test]
async fn a_session_is_bound_to_the_struct_identitys_subject() {
    let jwt = TestJwt::new();
    let router = secured_struct_app(&jwt).await;
    let alice = bearer(&jwt, "alice", &[]);
    let bob = bearer(&jwt, "bob", &[]);
    let session =
        support::initialize_with_headers(&router, "/mcp", &[("authorization", &alice)]).await;

    let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
    let replayed = support::post_with_headers(
        &router,
        "/mcp",
        Some(&session),
        &[("authorization", &bob)],
        &list,
    )
    .await;
    assert_eq!(
        replayed.status,
        StatusCode::NOT_FOUND,
        "{}",
        replayed.raw_body
    );
    assert!(replayed.messages.is_empty(), "{}", replayed.raw_body);
    let unknown = support::post_with_headers(
        &router,
        "/mcp",
        Some("no-such-session"),
        &[("authorization", &bob)],
        &list,
    )
    .await;
    assert_eq!(unknown.status, replayed.status);
    assert_eq!(unknown.raw_body, replayed.raw_body);

    // The owner is unaffected by the refused replay.
    let message = support::tools_call_with_headers(
        &router,
        "/mcp",
        &session,
        &[("authorization", &alice)],
        "whoami",
        json!({}),
    )
    .await;
    assert_eq!(text(&message), "alice", "{message}");
}

#[controller]
struct OptionalSecuredTools {
    #[inject(identity)]
    user: Option<AuthenticatedUser>,
}

#[routes]
impl OptionalSecuredTools {
    #[tool]
    async fn maybe_whoami(&self) -> String {
        self.user
            .as_ref()
            .map_or_else(|| "anonymous".to_string(), |u| u.sub.clone())
    }
}

/// With an optional struct identity the binding is exact: a session opened
/// by a subject is a 404 for an anonymous request and for another subject;
/// an anonymous session is a 404 for any subject. Both 404s are rmcp's own
/// "unknown session" answer.
#[r2e_core::test]
async fn an_optional_struct_identity_binds_the_session_exactly() {
    let jwt = TestJwt::new();
    let router = AppBuilder::new()
        .provide(Arc::new(jwt.claims_validator()))
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<OptionalSecuredTools>()
        .build();
    let alice = bearer(&jwt, "alice", &[]);
    let bob = bearer(&jwt, "bob", &[]);
    let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
    let unknown =
        support::post_with_headers(&router, "/mcp", Some("no-such-session"), &[], &list).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.raw_body);

    // alice's session: not for an anonymous request, not for bob.
    let owned =
        support::initialize_with_headers(&router, "/mcp", &[("authorization", &alice)]).await;
    for headers in [&[][..], &[("authorization", bob.as_str())][..]] {
        let refused = support::post_with_headers(&router, "/mcp", Some(&owned), headers, &list).await;
        assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.raw_body);
        assert_eq!(refused.raw_body, unknown.raw_body);
    }
    let message = support::tools_call_with_headers(
        &router,
        "/mcp",
        &owned,
        &[("authorization", &alice)],
        "maybe_whoami",
        json!({}),
    )
    .await;
    assert_eq!(text(&message), "alice", "{message}");

    // An anonymous session: not for alice.
    let anonymous = support::initialize(&router, "/mcp").await;
    let refused = support::post_with_headers(
        &router,
        "/mcp",
        Some(&anonymous),
        &[("authorization", &alice)],
        &list,
    )
    .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.raw_body);
    assert_eq!(refused.raw_body, unknown.raw_body);
    let message =
        support::tools_call(&router, "/mcp", &anonymous, "maybe_whoami", json!({})).await;
    assert_eq!(text(&message), "anonymous", "{message}");
}

// ── What the layer learns from the identity type ───────────────────────────

/// The caller probe picks the richest view the identity type offers, by
/// autoref priority: roles when `RoleBasedIdentity`, the subject alone when
/// only `Identity`, nothing otherwise — none of the traits being required.
#[test]
fn the_caller_probe_prefers_roles_then_subject_then_nothing() {
    use r2e_mcp::__macro_support::{
        CallerProbe, CallerViaIdentity as _, CallerViaNone as _, CallerViaRoles as _,
    };
    use r2e_security::RoleBasedIdentity;

    struct SubjectOnly;
    impl Identity for SubjectOnly {
        fn sub(&self) -> &str {
            "subject-only"
        }
    }
    struct WithRoles(Vec<String>);
    impl Identity for WithRoles {
        fn sub(&self) -> &str {
            "with-roles"
        }
    }
    impl RoleBasedIdentity for WithRoles {
        fn roles(&self) -> &[String] {
            &self.0
        }
    }

    let caller = (&&CallerProbe::<WithRoles>::new()).caller(Some(&WithRoles(vec!["admin".into()])));
    let caller = caller.expect("a RoleBasedIdentity reports a caller");
    assert_eq!(
        (caller.sub.as_str(), caller.roles.as_slice()),
        ("with-roles", &["admin".to_string()][..])
    );

    let caller = (&&CallerProbe::<SubjectOnly>::new()).caller(Some(&SubjectOnly));
    let caller = caller.expect("an Identity reports a caller");
    assert_eq!(
        (caller.sub.as_str(), caller.roles.len()),
        ("subject-only", 0)
    );

    // The `&&` is the autoref ladder itself (the probe picks the impl by
    // receiver depth), not a borrow to simplify.
    #[allow(clippy::needless_borrow)]
    let anonymous = (&&CallerProbe::<AppUser>::new())
        .caller(Some(&AppUser("x".into())))
        .is_none();
    assert!(anonymous);
    assert!((&&CallerProbe::<WithRoles>::new()).caller(None).is_none());
}
