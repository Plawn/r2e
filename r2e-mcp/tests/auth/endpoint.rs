//! The struct-level request scope under `mcp.auth`: the auth layer runs
//! first and deposits the principal, then the endpoint identity layer runs
//! every service's `RequestData::extract`.
//!
//! * `#[inject(identity)] principal: McpPrincipal` on the struct: extracted
//!   from the deposit (no second validation), `#[roles]` gating and list
//!   filtering through the principal, session binding left to the auth
//!   layer (one binding, not two);
//! * an app-defined struct identity rejected with a bare 401 gets the
//!   `mcp.auth` challenge appended, so an OAuth client still discovers the
//!   IdP from a rejection the extractor rendered itself;
//! * a struct identity whose rejection already carries a `Bearer` challenge
//!   (`AuthenticatedUser`'s RFC 6750 `error="invalid_token"`, #1173) gets
//!   `resource_metadata` merged into that one challenge (RFC 9728 §5.1) —
//!   never a second `Bearer` value — and a `Basic` challenge is not
//!   duplicated.

use std::sync::Arc;

use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::header::{HeaderMap, HeaderValue, Parts, WWW_AUTHENTICATE};
use r2e_core::http::{Body, Routes, StatusCode};
use r2e_core::prelude::*;
use r2e_core::{AppBuilder, Rejection, RejectionKind};
use r2e_mcp::auth::{McpAuthConfig, McpBasicAuthConfig};
use r2e_mcp::{AppBuilderMcpExt, McpPrincipal, McpServer};
use r2e_security::AuthenticatedUser;
use r2e_test::TestJwt;
use serde_json::json;

use crate::fixtures::{
    initialize_auth, offline_auth, pinned, post_auth, test_jwt, tools_list_auth, PRM_URL,
};
use crate::support;

#[controller]
struct PrincipalTools {
    #[inject(identity)]
    principal: McpPrincipal,
}

#[routes]
impl PrincipalTools {
    #[tool]
    async fn whoami(&self) -> String {
        self.principal.user.sub.clone()
    }

    #[tool]
    #[roles("admin")]
    async fn admin_only(&self) -> String {
        format!("admin:{}", self.principal.user.sub)
    }
}

/// An app identity read from a header, independent of the bearer.
#[derive(Clone)]
struct Tenant(String);

const MISSING_TENANT: &str = "missing x-tenant header";

impl<S: Send + Sync> FromRequestParts<S> for Tenant {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get("x-tenant")
            .and_then(|v| v.to_str().ok())
            .map(|t| Tenant(t.to_owned()))
            .ok_or_else(|| Rejection::new(RejectionKind::Unauthenticated, MISSING_TENANT))
    }
}

#[controller]
struct TenantTools {
    #[inject(identity)]
    tenant: Tenant,
}

#[routes]
impl TenantTools {
    #[tool]
    async fn tenant(&self) -> String {
        self.tenant.0.clone()
    }
}

async fn principal_app() -> Routes {
    AppBuilder::new()
        .plugin(
            McpServer::new()
                .with_auth(offline_auth())
                .with_token_validator(pinned(&test_jwt())),
        )
        .build_state()
        .await
        .register_mcp_service::<PrincipalTools>()
        .build()
}

async fn tenant_app() -> Routes {
    AppBuilder::new()
        .plugin(
            McpServer::new()
                .with_auth(offline_auth())
                .with_token_validator(pinned(&test_jwt())),
        )
        .build_state()
        .await
        .register_mcp_service::<TenantTools>()
        .build()
}

fn token(sub: &str, roles: &[&str]) -> String {
    test_jwt().token(sub, roles)
}

#[r2e_core::test]
async fn a_struct_principal_is_the_auth_layers_deposit() {
    let router = principal_app().await;

    // No bearer: the auth layer answers first, with its challenge.
    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(
        response.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        response.raw_body
    );

    let alice = token("alice", &[]);
    let session = initialize_auth(&router, "/mcp", &alice).await;
    let tools = support::tool_names(&tools_list_auth(&router, "/mcp", &session, &alice).await);
    assert_eq!(tools, vec!["whoami".to_string()], "{tools:?}");
    let message =
        crate::fixtures::tools_call_auth(&router, "/mcp", &session, &alice, "whoami", json!({}))
            .await;
    assert_eq!(
        message["result"]["content"][0]["text"], "alice",
        "{message}"
    );
    let message = crate::fixtures::tools_call_auth(
        &router,
        "/mcp",
        &session,
        &alice,
        "admin_only",
        json!({}),
    )
    .await;
    assert_eq!(message["error"]["data"], "forbidden", "{message}");

    let root = token("root", &["admin"]);
    let session = initialize_auth(&router, "/mcp", &root).await;
    let tools = support::tool_names(&tools_list_auth(&router, "/mcp", &session, &root).await);
    assert_eq!(
        tools,
        vec!["admin_only".to_string(), "whoami".to_string()],
        "{tools:?}"
    );
    let message =
        crate::fixtures::tools_call_auth(&router, "/mcp", &session, &root, "admin_only", json!({}))
            .await;
    assert_eq!(
        message["result"]["content"][0]["text"], "admin:root",
        "{message}"
    );

    // Session binding is the auth layer's: alice's session is a 404 for root.
    let session = initialize_auth(&router, "/mcp", &alice).await;
    let replayed = post_auth(
        &router,
        "/mcp",
        Some(&session),
        &root,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    assert_eq!(
        replayed.status,
        StatusCode::NOT_FOUND,
        "{}",
        replayed.raw_body
    );
}

#[r2e_core::test]
async fn a_rejected_struct_identity_carries_the_auth_challenge() {
    let router = tenant_app().await;
    let alice = token("alice", &[]);
    let bearer = format!("Bearer {alice}");

    // A valid bearer, no tenant header: the scope's own 401, with the
    // resource-metadata challenge appended by the identity layer.
    let (status, headers, body) = crate::fixtures::send(
        &router,
        "POST",
        "/mcp",
        &[
            ("authorization", bearer.as_str()),
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
        ],
        Body::from(support::initialize_body().to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(body.contains(MISSING_TENANT), "{body}");
    let challenge = headers
        .get("www-authenticate")
        .expect("challenge appended to the scope's 401")
        .to_str()
        .unwrap();
    assert_eq!(challenge, format!("Bearer resource_metadata=\"{PRM_URL}\""));

    // Both present: the member reads the struct identity.
    let (status, headers, body) = crate::fixtures::send(
        &router,
        "POST",
        "/mcp",
        &[
            ("authorization", bearer.as_str()),
            ("x-tenant", "acme"),
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
        ],
        Body::from(support::initialize_body().to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let session = headers
        .get("mcp-session-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let notified = support::post_with_headers(
        &router,
        "/mcp",
        Some(&session),
        &[("authorization", bearer.as_str()), ("x-tenant", "acme")],
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    assert_eq!(
        notified.status,
        StatusCode::ACCEPTED,
        "{}",
        notified.raw_body
    );
    let message = support::tools_call_with_headers(
        &router,
        "/mcp",
        &session,
        &[("authorization", bearer.as_str()), ("x-tenant", "acme")],
        "tenant",
        json!({}),
    )
    .await;
    assert_eq!(message["result"]["content"][0]["text"], "acme", "{message}");
}

// ── #1173: challenges already on the rejection are completed, not doubled ──

/// A struct-level `AuthenticatedUser` validated by a bean the auth layer
/// does not know about: the auth layer accepts the bearer, the scope's
/// extractor refuses it.
#[controller]
struct StrictTools {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl StrictTools {
    #[tool]
    async fn whoami(&self) -> String {
        self.user.sub.clone()
    }
}

/// An identity rejecting with the bare `Bearer` challenge and an explicit
/// `Basic` one — the shape of a hand-written extractor advertising both.
#[derive(Clone)]
struct Dual(String);

impl<S: Send + Sync> FromRequestParts<S> for Dual {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get("x-dual")
            .and_then(|v| v.to_str().ok())
            .map(|t| Dual(t.to_owned()))
            .ok_or_else(|| {
                Rejection::new(RejectionKind::Unauthenticated, "missing x-dual header")
                    .header(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))
                    .header(
                        WWW_AUTHENTICATE,
                        HeaderValue::from_static(r#"Basic realm="legacy""#),
                    )
            })
    }
}

#[controller]
struct DualTools {
    #[inject(identity)]
    dual: Dual,
}

#[routes]
impl DualTools {
    #[tool]
    async fn dual(&self) -> String {
        self.dual.0.clone()
    }
}

fn challenges(headers: &HeaderMap) -> Vec<String> {
    headers
        .get_all(WWW_AUTHENTICATE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect()
}

async fn initialize_raw(router: &Routes, headers: &[(&str, &str)]) -> (StatusCode, HeaderMap, String) {
    let mut all = vec![
        ("content-type", "application/json"),
        ("accept", "application/json, text/event-stream"),
    ];
    all.extend_from_slice(headers);
    crate::fixtures::send(
        router,
        "POST",
        "/mcp",
        &all,
        Body::from(support::initialize_body().to_string()),
    )
    .await
}

#[r2e_core::test]
async fn an_invalid_token_challenge_from_the_scope_gains_the_resource_metadata() {
    // The scope's validator is a different TestJwt (other secret): every
    // bearer the auth layer accepts is `invalid_token` for the scope.
    let router = AppBuilder::new()
        .provide(Arc::new(TestJwt::new().claims_validator()))
        .plugin(
            McpServer::new()
                .with_auth(offline_auth())
                .with_token_validator(pinned(&test_jwt())),
        )
        .build_state()
        .await
        .register_mcp_service::<StrictTools>()
        .build();
    let bearer = format!("Bearer {}", token("alice", &[]));

    let (status, headers, body) = initialize_raw(&router, &[("authorization", &bearer)]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(
        challenges(&headers),
        vec![format!(
            "Bearer error=\"invalid_token\", error_description=\"The access token is invalid\", \
             resource_metadata=\"{PRM_URL}\""
        )],
        "one Bearer challenge: the extractor's, completed with the PRM URL"
    );
}

#[r2e_core::test]
async fn bearer_and_basic_challenges_from_the_scope_are_completed_not_duplicated() {
    let auth = McpAuthConfig {
        basic: Some(McpBasicAuthConfig {
            username: "svc".into(),
            password: "pw".into(),
            realm: None,
            roles: None,
            scopes: None,
        }),
        ..offline_auth()
    };
    let router = AppBuilder::new()
        .plugin(
            McpServer::new()
                .with_auth(auth)
                .with_token_validator(pinned(&test_jwt())),
        )
        .build_state()
        .await
        .register_mcp_service::<DualTools>()
        .build();
    let bearer = format!("Bearer {}", token("alice", &[]));

    let (status, headers, body) = initialize_raw(&router, &[("authorization", &bearer)]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(
        challenges(&headers),
        vec![
            format!("Bearer resource_metadata=\"{PRM_URL}\""),
            r#"Basic realm="legacy""#.to_string(),
        ],
        "the bare Bearer gains the PRM URL; the extractor's Basic wins over mcp.auth's"
    );
}

/// An identity rejecting with whatever `WWW-Authenticate` values the request
/// advertises in `x-challenge` headers — drives the RFC 9110 parsing cases.
#[derive(Clone)]
struct Advertised;

impl<S: Send + Sync> FromRequestParts<S> for Advertised {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let mut rejection = Rejection::new(RejectionKind::Unauthenticated, "advertised");
        for value in parts.headers.get_all("x-challenge") {
            rejection = rejection.header(WWW_AUTHENTICATE, value.clone());
        }
        Err(rejection)
    }
}

#[controller]
struct AdvertisedTools {
    #[inject(identity)]
    advertised: Advertised,
}

#[routes]
impl AdvertisedTools {
    #[tool]
    async fn never(&self) -> String {
        String::new()
    }
}

#[r2e_core::test]
async fn challenges_are_parsed_per_rfc_9110_before_completion() {
    let auth = McpAuthConfig {
        basic: Some(McpBasicAuthConfig {
            username: "svc".into(),
            password: "pw".into(),
            realm: Some("svc".into()),
            roles: None,
            scopes: None,
        }),
        ..offline_auth()
    };
    let router = AppBuilder::new()
        .plugin(
            McpServer::new()
                .with_auth(auth)
                .with_token_validator(pinned(&test_jwt())),
        )
        .build_state()
        .await
        .register_mcp_service::<AdvertisedTools>()
        .build();
    let prm = format!("resource_metadata=\"{PRM_URL}\"");
    let basic = r#"Basic realm="svc", charset="UTF-8""#;

    let cases: Vec<(&str, Vec<&str>, Vec<String>)> = vec![
        (
            "two challenges in one value: the Bearer gets the metadata, not the Basic",
            vec![r#"Bearer error="invalid_token", Basic realm="legacy""#],
            vec![format!("Bearer error=\"invalid_token\", {prm}"), r#"Basic realm="legacy""#.into()],
        ),
        (
            "same, Basic first",
            vec![r#"Basic realm="legacy", Bearer realm="api""#],
            vec![r#"Basic realm="legacy""#.into(), format!("Bearer realm=\"api\", {prm}")],
        ),
        (
            "resource_metadata inside a quoted value is not the parameter",
            vec![r#"Bearer realm="resource_metadata=legacy""#],
            vec![format!("Bearer realm=\"resource_metadata=legacy\", {prm}"), basic.into()],
        ),
        (
            "parameter names are case-insensitive",
            vec![r#"Bearer RESOURCE_METADATA="https://example.org/meta""#],
            vec![r#"Bearer RESOURCE_METADATA="https://example.org/meta""#.into(), basic.into()],
        ),
        (
            "whitespace around = is accepted",
            vec![r#"Bearer resource_metadata = "https://example.org/meta""#],
            vec![r#"Bearer resource_metadata = "https://example.org/meta""#.into(), basic.into()],
        ),
        (
            "a quoted comma does not split the challenge",
            vec![r#"Bearer error_description="expired, renew it""#],
            vec![format!("Bearer error_description=\"expired, renew it\", {prm}"), basic.into()],
        ),
        (
            "an escaped quote stays inside the quoted string",
            vec![r#"Bearer error_description="say \"hi\", ok""#],
            vec![format!("Bearer error_description=\"say \\\"hi\\\", ok\", {prm}"), basic.into()],
        ),
        (
            "an unknown scheme is kept and both mcp.auth challenges are appended",
            vec!["Negotiate"],
            vec!["Negotiate".into(), format!("Bearer {prm}"), basic.into()],
        ),
        (
            "a token68 challenge is a challenge, not a parameter",
            vec![r#"Bearer realm="api""#, "Negotiate YWJj="],
            vec![format!("Bearer realm=\"api\", {prm}"), "Negotiate YWJj=".into(), basic.into()],
        ),
        (
            "a parameter before any scheme is kept verbatim",
            vec![r#"realm="orphan""#],
            vec![r#"realm="orphan""#.into(), format!("Bearer {prm}"), basic.into()],
        ),
        (
            "scheme matching is case-insensitive",
            vec!["bearer", r#"BASIC realm="x""#],
            vec![format!("bearer {prm}"), r#"BASIC realm="x""#.into()],
        ),
    ];

    // A bearer the auth layer accepts: the scope's extractor is what rejects.
    let bearer = format!("Bearer {}", token("alice", &[]));
    for (case, advertised, expected) in cases {
        let mut headers: Vec<(&str, &str)> = vec![("authorization", &bearer)];
        headers.extend(advertised.iter().map(|v| ("x-challenge", *v)));
        let (status, headers, body) = initialize_raw(&router, &headers).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{case}: {body}");
        assert_eq!(challenges(&headers), expected, "{case}");
    }
}
