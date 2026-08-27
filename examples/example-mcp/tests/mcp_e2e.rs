//! Smoke tests for the example blueprint itself. Protocol behavior, schemas,
//! decorators and auth edge cases live in `r2e-mcp/tests`; this target only
//! proves the facade wiring shown to users.

use example_mcp::{CalcService, CallLog, MathTools, McpApp};
use r2e::prelude::*;
use r2e::r2e_mcp::testing::pin_mcp_validator;
use r2e_test::{TestApp, TestJwt};
use serde_json::{json, Value};

const RESOURCE: &str = "http://localhost:3000/mcp";

fn jwt() -> TestJwt {
    TestJwt::for_resource(RESOURCE)
}

/// The documented no-Docker authenticated variant of the example.
struct SecureMcpApp;

impl App for SecureMcpApp {
    type Env = ();

    async fn setup() {}

    async fn build(b: AppBuilder, _env: ()) -> impl BootableApp {
        pin_mcp_validator(b, &jwt(), RESOURCE)
            .load_config::<()>()
            .plugin(McpServer::new().with_name("example-mcp-secure"))
            .provide(CalcService)
            .provide(CallLog::default())
            .build_state()
            .await
            .register_mcp_service::<MathTools>()
    }
}

fn response_message(response: r2e_test::TestResponse) -> Value {
    let messages: Vec<Value> = response
        .text()
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|data| !data.is_empty())
        .map(|data| serde_json::from_str(data).expect("SSE data event is not JSON"))
        .collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    messages.into_iter().next().unwrap()
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
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    request.send().await
}

async fn initialize(app: &TestApp, token: Option<&str>) -> (String, Value) {
    let response = mcp_post(
        app,
        None,
        token,
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "example-mcp-smoke", "version": "0.0.0" }
            }
        }),
    )
    .await;
    response.assert_ok();
    let session = response
        .header("mcp-session-id")
        .expect("no Mcp-Session-Id header")
        .to_string();
    let initialized = response_message(response);
    mcp_post(
        app,
        Some(&session),
        token,
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    (session, initialized)
}

async fn call_add(app: &TestApp, session: &str, token: Option<&str>) -> Value {
    let response = mcp_post(
        app,
        Some(session),
        token,
        &json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "add", "arguments": { "a": 2.0, "b": 3.0 } }
        }),
    )
    .await;
    response.assert_ok();
    response_message(response)
}

#[r2e::test(app = McpApp)]
async fn facade_blueprint_serves_http_and_all_mcp_families(app: TestApp) {
    let (session, initialized) = initialize(&app, None).await;
    let capabilities = &initialized["result"]["capabilities"];
    assert!(capabilities.get("tools").is_some(), "{capabilities}");
    assert!(capabilities.get("resources").is_some(), "{capabilities}");
    assert!(capabilities.get("prompts").is_some(), "{capabilities}");

    let message = call_add(&app, &session, None).await;
    assert_eq!(message["result"]["structuredContent"]["value"], 5.0);

    let response = app.get("/api/calc/add/2/3").send().await;
    response.assert_ok();
    let body: Value = response.json();
    assert_eq!(body["value"], 5.0);
}

#[r2e::test(app = SecureMcpApp)]
async fn facade_testing_wiring_challenges_then_authenticates(app: TestApp) {
    let response = mcp_post(
        &app,
        None,
        None,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
    )
    .await;
    response.assert_unauthorized();
    assert!(response
        .header("www-authenticate")
        .is_some_and(|value| value.contains("resource_metadata=")));

    let token = jwt().token("alice", &[]);
    let (session, _) = initialize(&app, Some(&token)).await;
    let message = call_add(&app, &session, Some(&token)).await;
    assert_eq!(message["result"]["structuredContent"]["value"], 5.0);
}
