//! Auth observability (#1206): the boot warning for a scopeless DCR shim,
//! the DCR `scope` echo, one cause-carrying line per token rejection, one
//! line per MCP request and per DCR call — and never a token in a log.
//!
//! Every test installs a thread-local subscriber on a `current_thread`
//! runtime (the `server::mount` idiom): boot and requests are polled on the
//! test thread, so captures cannot interleave with the other tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use r2e_core::http::{Body, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::auth::McpAuthConfig;
use r2e_mcp::{AppBuilderMcpExt, McpServer};
use serde_json::Value;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::Registry;

use crate::fixtures::{
    initialize_auth, offline_auth, post_auth, secured_app, secured_app_with, send, test_jwt,
};
use crate::support;

// ── Capture ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Line {
    level: Level,
    fields: HashMap<String, String>,
}

impl Line {
    fn message(&self) -> &str {
        self.field("message")
    }

    fn field(&self, name: &str) -> &str {
        self.fields.get(name).map(String::as_str).unwrap_or("")
    }
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<Line>>>);

struct Fields(HashMap<String, String>);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
}

impl<S: Subscriber> Layer<S> for Captured {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields(HashMap::new());
        event.record(&mut fields);
        self.0.lock().unwrap().push(Line {
            level: *event.metadata().level(),
            fields: fields.0,
        });
    }
}

impl Captured {
    fn install(&self) -> tracing::subscriber::DefaultGuard {
        tracing::subscriber::set_default(Registry::default().with(self.clone()))
    }

    fn take(&self) -> Vec<Line> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }

    fn with_message(&self, message: &str) -> Vec<Line> {
        self.take()
            .into_iter()
            .filter(|line| line.message() == message)
            .collect()
    }
}

const REJECTED: &str = "MCP request rejected";
const NO_CREDENTIALS: &str = "MCP request without credentials; answering with the OAuth challenge";
const REQUEST: &str = "MCP request";
const SHIM_SCOPELESS: &str = "mcp.auth: the DCR shim is active but the protected-resource metadata";

fn shim_auth() -> McpAuthConfig {
    McpAuthConfig {
        public_client_id: Some("mcp-public".into()),
        ..offline_auth()
    }
}

/// POST `initialize` with a raw `Authorization` value; returns the
/// rejection lines.
async fn rejected_with(captured: &Captured, authorization: &str) -> Vec<Line> {
    let router = secured_app().await;
    captured.take();
    let response = support::post_with_headers(
        &router,
        "/mcp",
        None,
        &[("authorization", authorization)],
        &support::initialize_body(),
    )
    .await;
    assert!(
        response.status.is_client_error(),
        "{authorization}: {}",
        response.status
    );
    captured.with_message(REJECTED)
}

fn assert_one_warn(lines: &[Line], cause: &str) {
    assert_eq!(lines.len(), 1, "one rejection line expected: {lines:#?}");
    assert_eq!(lines[0].level, Level::WARN, "{lines:#?}");
    assert_eq!(lines[0].field("cause"), cause, "{lines:#?}");
}

// ── 1. Boot warning: DCR shim with no advertised scope ─────────────────────

#[r2e_core::test(flavor = "current_thread")]
async fn scopeless_shim_warns_at_boot_naming_the_key() {
    let captured = Captured::default();
    let _guard = captured.install();
    secured_app_with(shim_auth()).await;

    let lines: Vec<Line> = captured
        .take()
        .into_iter()
        .filter(|line| line.message().starts_with(SHIM_SCOPELESS))
        .collect();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(lines[0].level, Level::WARN);
    assert!(
        lines[0].message().contains("mcp.auth.scopes-supported"),
        "{}",
        lines[0].message()
    );
}

#[r2e_core::test(flavor = "current_thread")]
async fn shim_with_scopes_does_not_warn() {
    let captured = Captured::default();
    let _guard = captured.install();
    secured_app_with(McpAuthConfig {
        scopes_supported: Some(vec!["mcp:read".into()]),
        ..shim_auth()
    })
    .await;

    assert!(!captured
        .take()
        .iter()
        .any(|line| line.message().starts_with(SHIM_SCOPELESS)));
}

// ── 2 + 4. DCR: `scope` echo and one line per call ─────────────────────────

async fn register(router: &r2e_core::http::Routes, body: &str) -> (StatusCode, Value) {
    let (status, _, raw) = send(
        router,
        "POST",
        "/mcp/oauth/register",
        &[("content-type", "application/json")],
        Body::from(body.to_string()),
    )
    .await;
    let parsed: Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("not JSON ({e}): {raw}"));
    (status, parsed)
}

#[r2e_core::test(flavor = "current_thread")]
async fn dcr_echoes_the_advertised_scopes_and_logs_one_line() {
    let captured = Captured::default();
    let _guard = captured.install();
    let router = secured_app_with(McpAuthConfig {
        scopes_supported: Some(vec!["mcp:read".into(), "mcp:write".into()]),
        ..shim_auth()
    })
    .await;
    captured.take();

    let (status, doc) = register(
        &router,
        r#"{"client_name":"probe","redirect_uris":["http://127.0.0.1:5555/cb"]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{doc}");
    assert_eq!(doc["scope"], "mcp:read mcp:write");

    let lines: Vec<Line> = captured
        .take()
        .into_iter()
        .filter(|line| line.message().starts_with("DCR shim"))
        .collect();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(lines[0].level, Level::INFO);
    assert_eq!(lines[0].field("status"), "201");
    assert_eq!(lines[0].field("scope"), "mcp:read mcp:write");
}

#[r2e_core::test(flavor = "current_thread")]
async fn dcr_without_scopes_omits_scope_and_refusals_log_a_warn() {
    let captured = Captured::default();
    let _guard = captured.install();
    let router = secured_app_with(shim_auth()).await;
    captured.take();

    let (status, doc) =
        register(&router, r#"{"redirect_uris":["http://127.0.0.1:5555/cb"]}"#).await;
    assert_eq!(status, StatusCode::CREATED, "{doc}");
    assert!(doc.get("scope").is_none(), "{doc}");
    captured.take();

    let (status, _) = register(&router, "not json").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let lines: Vec<Line> = captured
        .take()
        .into_iter()
        .filter(|line| line.message().starts_with("DCR shim"))
        .collect();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(lines[0].level, Level::WARN);
    assert_eq!(lines[0].field("status"), "400");
}

// ── 3. Token rejections, with their cause ──────────────────────────────────

#[r2e_core::test(flavor = "current_thread")]
async fn missing_credentials_is_a_debug_line_not_a_warn() {
    let captured = Captured::default();
    let _guard = captured.install();
    let router = secured_app().await;
    captured.take();

    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    let lines = captured.take();
    assert!(
        !lines.iter().any(|line| line.message() == REJECTED),
        "{lines:#?}"
    );
    let debug: Vec<&Line> = lines
        .iter()
        .filter(|line| line.message() == NO_CREDENTIALS)
        .collect();
    assert_eq!(debug.len(), 1, "{lines:#?}");
    assert_eq!(debug[0].level, Level::DEBUG);
}

#[r2e_core::test(flavor = "current_thread")]
async fn each_rejection_logs_its_cause() {
    let captured = Captured::default();
    let _guard = captured.install();
    let jwt = test_jwt();

    let cases = [
        ("Bearer".to_string(), "malformed_header"),
        // OAuth-only endpoint: any non-Bearer scheme is a malformed header.
        ("Token abc".to_string(), "malformed_header"),
        (
            format!("Bearer {}", r2e_test::TestJwt::malformed_token()),
            "malformed_token",
        ),
        (
            format!("Bearer {}", jwt.token_builder("alice").expired().build()),
            "token_expired",
        ),
        (
            format!("Bearer {}", jwt.wrong_audience_token("alice")),
            "claims",
        ),
        (
            format!("Bearer {}", jwt.wrong_issuer_token("alice")),
            "claims",
        ),
        (
            format!("Bearer {}", jwt.wrong_algorithm_token("alice")),
            "algorithm_refused",
        ),
        (
            format!(
                "Bearer {}",
                r2e_test::TestJwt::with_config(
                    b"another-secret",
                    crate::fixtures::ISSUER,
                    crate::fixtures::RESOURCE
                )
                .token("alice", &[])
            ),
            "invalid_signature",
        ),
    ];
    for (authorization, cause) in cases {
        let lines = rejected_with(&captured, &authorization).await;
        assert_one_warn(&lines, cause);
        // The token never reaches the log, whatever the field.
        if let Some(token) = authorization.strip_prefix("Bearer ")
            && token.len() > 8
        {
            for value in lines[0].fields.values() {
                assert!(!value.contains(token), "token leaked into {lines:#?}");
            }
        }
    }
}

#[r2e_core::test(flavor = "current_thread")]
async fn insufficient_scope_logs_the_missing_scopes() {
    let captured = Captured::default();
    let _guard = captured.install();
    let router = secured_app_with(McpAuthConfig {
        required_scopes: Some(vec!["mcp:read".into(), "mcp:write".into()]),
        ..offline_auth()
    })
    .await;
    captured.take();

    let token = test_jwt()
        .token_builder("alice")
        .scopes(&["mcp:read"])
        .build();
    let response = post_auth(&router, "/mcp", None, &token, &support::initialize_body()).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);

    let lines = captured.with_message(REJECTED);
    assert_one_warn(&lines, "insufficient_scope");
    assert!(lines[0].field("status").contains("403"), "{lines:#?}");
    assert!(lines[0].field("detail").contains("mcp:write"), "{lines:#?}");
}

// ── 4. One line per MCP request ────────────────────────────────────────────

#[r2e_core::test(flavor = "current_thread")]
async fn each_mcp_request_logs_method_status_and_auth_outcome() {
    let captured = Captured::default();
    let _guard = captured.install();
    let router = secured_app().await;
    captured.take();

    let token = test_jwt().token("alice", &[]);
    let session = initialize_auth(&router, "/mcp", &token).await;
    let lines = captured.with_message(REQUEST);
    assert_eq!(lines.len(), 2, "{lines:#?}");
    assert!(lines.iter().all(|line| line.level == Level::DEBUG));
    assert_eq!(lines[0].field("rpc_method"), "initialize");
    assert_eq!(lines[0].field("status"), "200");
    assert_eq!(lines[0].field("auth"), "authenticated");
    assert_eq!(lines[1].field("rpc_method"), "notifications/initialized");
    assert_eq!(lines[1].field("status"), "202");

    // A rejected request: no body peek, the cause as the outcome.
    let response = post_auth(
        &router,
        "/mcp",
        Some(&session),
        "not-a-jwt",
        &support::initialize_body(),
    )
    .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    let lines = captured.with_message(REQUEST);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(lines[0].field("rpc_method"), "-");
    assert_eq!(lines[0].field("status"), "401");
    assert_eq!(lines[0].field("auth"), "malformed_token");
}

/// A request rmcp refuses on its headers never has its body read: the log
/// line says so (`-`) rather than buffering a body nobody parses.
#[r2e_core::test(flavor = "current_thread")]
async fn body_rejected_on_headers_is_never_read() {
    let captured = Captured::default();
    let _guard = captured.install();
    let router = secured_app().await;
    captured.take();

    let token = test_jwt().token("alice", &[]);
    let bearer = format!("Bearer {token}");
    let (status, _, _) = send(
        &router,
        "POST",
        "/mcp",
        &[
            ("authorization", bearer.as_str()),
            ("content-type", "application/json"),
            // Missing `text/event-stream`: rmcp answers 406 before the body.
            ("accept", "application/json"),
        ],
        Body::from(support::initialize_body().to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_ACCEPTABLE);
    let lines = captured.with_message(REQUEST);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(lines[0].field("rpc_method"), "-");
    assert_eq!(lines[0].field("auth"), "authenticated");
}

/// The DCR line is bounded whatever the (unauthenticated) client sends:
/// counts plus a clipped sample, never the full request.
#[r2e_core::test(flavor = "current_thread")]
async fn dcr_log_line_is_bounded() {
    let captured = Captured::default();
    let _guard = captured.install();
    let router = secured_app_with(shim_auth()).await;
    captured.take();

    // ~12 KiB: under the 16 KiB registration cap, far over any log budget.
    let long_name = "n".repeat(500);
    let uris: Vec<String> = (0..50)
        .map(|i| format!("https://evil.example/{}/{i}", "x".repeat(200)))
        .collect();
    let body = serde_json::json!({ "client_name": long_name, "redirect_uris": uris });
    let (status, doc) = register(&router, &body.to_string()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{doc}");

    let lines: Vec<Line> = captured
        .take()
        .into_iter()
        .filter(|line| line.message().starts_with("DCR shim"))
        .collect();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(lines[0].field("rejected_redirect_uris"), "50");
    assert!(lines[0].field("client_name").len() < 100, "{lines:#?}");
    let total: usize = lines[0].fields.values().map(String::len).sum();
    assert!(
        total < 1024,
        "log line not bounded ({total} bytes): {lines:#?}"
    );
}

/// No scope requirement: mountable without `mcp.auth`.
#[controller]
pub struct OpenTools;

#[mcp_routes]
impl OpenTools {
    /// Echo.
    #[tool]
    async fn echo(&self) -> String {
        "ok".into()
    }
}

#[r2e_core::test(flavor = "current_thread")]
async fn unauthenticated_server_logs_auth_none() {
    let captured = Captured::default();
    let _guard = captured.install();
    let router = AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<OpenTools>()
        .build();
    captured.take();

    support::initialize(&router, "/mcp").await;
    let lines = captured.with_message(REQUEST);
    assert_eq!(lines.len(), 2, "{lines:#?}");
    assert_eq!(lines[0].field("rpc_method"), "initialize");
    assert_eq!(lines[0].field("auth"), "none");
}
