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
//!   the `Arc<JwtClaimsValidator>` bean provided and a `TestJwt` bearer.

use std::sync::Arc;

use r2e_core::http::extract::{FromRequestParts, OptionalFromRequestParts};
use r2e_core::http::header::Parts;
use r2e_core::http::Routes;
use r2e_core::prelude::*;
use r2e_core::{AppBuilder, Rejection, RejectionKind};
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

#[mcp_routes]
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

#[tokio::test]
async fn an_app_identity_reaches_a_tool() {
    let router = app_identity_app().await;
    let session = support::initialize(&router, "/mcp").await;

    let message = call_as(&router, &session, Some("alice"), "hello").await;
    assert_eq!(text(&message), "hello alice", "{message}");

    let message = call_as(&router, &session, Some("alice"), "maybe_hello").await;
    assert_eq!(text(&message), "hello alice", "{message}");
}

#[tokio::test]
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

#[tokio::test]
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

#[mcp_routes]
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

#[tokio::test]
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
