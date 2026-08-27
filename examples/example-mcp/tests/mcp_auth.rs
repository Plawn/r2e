//! Authenticated MCP e2e: the documented no-Docker fast path
//! (`pin_mcp_validator` + `TestJwt::for_resource`) over the real example
//! service — challenge on missing token, PRM discovery, full authenticated
//! session, and the shared guard still applying on top of auth.

use example_mcp::MathTools;
use r2e::prelude::*;
use r2e::r2e_mcp::testing::pin_mcp_validator;
use r2e::r2e_mcp::AppBuilderMcpExt;
use r2e_test::{TestApp, TestJwt};
use serde_json::{json, Value};

/// The canonical resource URI the pinned validator binds tokens to
/// (`aud` = this, RFC 8707).
const RESOURCE: &str = "http://localhost:3000/mcp";

/// Deterministic: the same secret/issuer/audience in the blueprint and the
/// tests.
fn jwt() -> TestJwt {
    TestJwt::for_resource(RESOURCE)
}

// ── Secured blueprint ───────────────────────────────────────────────────
//
// Same beans and MCP service as `McpApp`, with auth pinned to the TestJwt —
// zero network I/O at boot (no discovery, no JWKS).

pub struct SecureMcpApp;

impl App for SecureMcpApp {
    type Env = ();

    async fn setup() {}

    async fn build(b: AppBuilder, _env: ()) -> impl BootableApp {
        pin_mcp_validator(b, &jwt(), RESOURCE)
            .load_config::<()>()
            .plugin(McpServer::new().with_name("example-mcp-secure"))
            .provide(example_mcp::CalcService)
            .provide(example_mcp::CallLog::default())
            .build_state()
            .await
            .register_mcp_service::<MathTools>()
    }
}

// ── Minimal authenticated MCP client over TestApp ───────────────────────

fn sse_messages(body: &str) -> Vec<Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|data| !data.is_empty())
        .map(|data| serde_json::from_str(data).expect("SSE data event is not JSON"))
        .collect()
}

async fn mcp_post(
    app: &TestApp,
    session: Option<&str>,
    token: Option<&str>,
    body: &Value,
) -> r2e_test::TestResponse {
    let mut request = app
        .post("/mcp")
        .header("host", "localhost")
        .header("accept", "application/json, text/event-stream")
        .json(body);
    if let Some(sid) = session {
        request = request.header("mcp-session-id", sid);
    }
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    request.send().await
}

async fn initialize(app: &TestApp, token: &str) -> String {
    let response = mcp_post(
        app,
        None,
        Some(token),
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "example-mcp-auth", "version": "0.0.0" }
            }
        }),
    )
    .await;
    response.assert_ok();
    let session = response
        .header("mcp-session-id")
        .expect("no Mcp-Session-Id header")
        .to_string();
    mcp_post(
        app,
        Some(&session),
        Some(token),
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    session
}

async fn tools_call(
    app: &TestApp,
    session: &str,
    token: &str,
    headers: &[(&'static str, &'static str)],
    name: &str,
    arguments: Value,
) -> Value {
    let mut request = app
        .post("/mcp")
        .header("host", "localhost")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", session)
        .header("authorization", format!("Bearer {token}"))
        .json(&json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }));
    for (header, value) in headers {
        request = request.header(*header, *value);
    }
    let response = request.send().await;
    response.assert_ok();
    let messages = sse_messages(&response.text());
    assert_eq!(messages.len(), 1, "{messages:?}");
    messages.into_iter().next().unwrap()
}

// ── Tests ───────────────────────────────────────────────────────────────

#[r2e::test(app = SecureMcpApp)]
async fn missing_token_is_challenged_with_resource_metadata(app: TestApp) {
    let response = mcp_post(
        &app,
        None,
        None,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
    )
    .await;
    response.assert_unauthorized();
    let challenge = response
        .header("www-authenticate")
        .expect("no WWW-Authenticate");
    assert_eq!(
        challenge,
        "Bearer resource_metadata=\"http://localhost:3000/.well-known/oauth-protected-resource/mcp\""
    );
}

#[r2e::test(app = SecureMcpApp)]
async fn protected_resource_metadata_is_public(app: TestApp) {
    let response = app
        .get("/.well-known/oauth-protected-resource/mcp")
        .send()
        .await;
    response.assert_ok();
    let doc: Value = response.json();
    assert_eq!(doc["resource"], RESOURCE);
    assert_eq!(doc["authorization_servers"], json!([jwt().issuer()]));
}

#[r2e::test(app = SecureMcpApp)]
async fn invalid_token_gets_invalid_token_error(app: TestApp) {
    let response = mcp_post(
        &app,
        None,
        Some(&TestJwt::malformed_token()),
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
    )
    .await;
    response.assert_unauthorized();
    let body: Value = response.json();
    assert_eq!(body["error"], "invalid_token");
}

#[r2e::test(app = SecureMcpApp)]
async fn authenticated_session_calls_tools(app: TestApp) {
    let token = jwt().token("alice", &[]);
    let session = initialize(&app, &token).await;
    let msg = tools_call(&app, &session, &token, &[], "add", json!({"a": 2.0, "b": 3.0})).await;
    assert_eq!(msg["result"]["structuredContent"]["value"], 5.0, "{msg}");
}

#[r2e::test(app = SecureMcpApp)]
async fn shared_guard_still_applies_on_top_of_auth(app: TestApp) {
    let token = jwt().token("alice", &[]);
    let session = initialize(&app, &token).await;

    // Authenticated but no API key → the ApiKeyGuard still denies.
    let denied = tools_call(&app, &session, &token, &[], "clear_log", json!({})).await;
    assert_eq!(denied["error"]["code"], -32600, "{denied}");

    // Token + API key → allowed.
    let ok = tools_call(
        &app,
        &session,
        &token,
        &[("x-api-key", "letmein")],
        "clear_log",
        json!({}),
    )
    .await;
    assert!(ok["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("cleared"), "{ok}");
}
