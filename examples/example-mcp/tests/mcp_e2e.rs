//! Smoke tests for the example blueprint itself. Protocol behavior, schemas,
//! decorators and auth edge cases live in `r2e-mcp/tests`; this target only
//! proves the facade wiring shown to users.
//!
//! MCP calls go through `app.mcp()` ([`McpTestExt`]): a real session on the
//! mounted endpoint, in process. Raw HTTP is kept only where the HTTP
//! envelope itself is the subject (the 401 challenge, a header-reading guard).

use example_mcp::{CalcService, CallLog, MathTools, McpApp};
use r2e::prelude::*;
use r2e::r2e_mcp::testing::{pin_mcp_validator, McpNotification, McpTestExt};
use r2e_test::{TestApp, TestJwt};
use serde::Deserialize;
use serde_json::{json, Value};

const RESOURCE: &str = "http://localhost:3000/mcp";

fn jwt() -> TestJwt {
    TestJwt::for_resource(RESOURCE)
}

/// What `add` / `sqrt` return (`CalcResult` on the server side).
#[derive(Deserialize)]
struct Calc {
    value: f64,
}

/// The documented no-Docker authenticated variant of the example.
struct SecureMcpApp;

impl App for SecureMcpApp {
    type Env = ();

    async fn setup() -> Result<(), BootError> {
        Ok(())
    }

    async fn build(b: AppBuilder, _env: ()) -> Result<impl BootableApp, BootError> {
        Ok({
            pin_mcp_validator(b, &jwt(), RESOURCE)
                .load_config::<()>()
                .plugin(McpServer::new().with_name("example-mcp-secure"))
                .provide(CalcService)
                .provide(CallLog::default())
                .build_state()
                .await
                .register_mcp_service::<MathTools>()
        })
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

async fn mcp_post(app: &TestApp, session: Option<&str>, body: &Value) -> r2e_test::TestResponse {
    let mut request = app
        .post("/mcp")
        .header("host", "localhost")
        .header("accept", "application/json, text/event-stream")
        .json(body);
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    request.send().await
}

async fn initialize(app: &TestApp) -> String {
    let response = mcp_post(
        app,
        None,
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
    mcp_post(
        app,
        Some(&session),
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    session
}

/// Two adapters over one `CalcService` bean: the `MathTools` MCP service
/// (its `add` tool) and the `CalcController` HTTP controller (`GET
/// /api/calc/add/{a}/{b}`).
#[r2e::test(app = McpApp)]
async fn facade_blueprint_serves_http_and_all_mcp_families(app: TestApp) {
    let mcp = app.mcp().await;
    assert!(mcp.tool_names().await.contains(&"add".to_string()));
    let resources = mcp.list_resources().await.unwrap();
    assert!(resources.iter().any(|r| r.uri == "r2e://calc/call-log"));
    let prompts = mcp.list_prompts().await.unwrap();
    assert!(prompts.iter().any(|p| p.name == "explain_division"));

    let sum: Calc = mcp
        .call_tool("add", json!({ "a": 2.0, "b": 3.0 }))
        .await
        .unwrap();
    assert_eq!(sum.value, 5.0);

    let response = app.get("/api/calc/add/2/3").send().await;
    response.assert_ok();
    let body: Value = response.json();
    assert_eq!(body["value"], 5.0);
}

#[r2e::test(app = SecureMcpApp)]
async fn facade_testing_wiring_challenges_then_authenticates(app: TestApp) {
    // The HTTP envelope: no token → 401 + the RFC 9728 challenge.
    let response = mcp_post(
        &app,
        None,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
    )
    .await;
    response.assert_unauthorized();
    assert!(response
        .header("www-authenticate")
        .is_some_and(|value| value.contains("resource_metadata=")));
    // The same refusal, in process.
    assert!(app.mcp().try_connect().await.is_err());

    // `as_user` mints the token from the app's `TestJwt`, bound to the
    // endpoint's resource — the same identity `.as_user()` gives over HTTP.
    let mcp = app.mcp().as_user("alice", &[]).await;
    let sum: Calc = mcp
        .call_tool("add", json!({ "a": 2.0, "b": 3.0 }))
        .await
        .unwrap();
    assert_eq!(sum.value, 5.0);
}

async fn call_clear_log(app: &TestApp, session: &str, api_key: Option<&str>) -> Value {
    let mut request = app
        .post("/mcp")
        .header("host", "localhost")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", session)
        .json(&json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "clear_log", "arguments": {} }
        }));
    if let Some(api_key) = api_key {
        request = request.header("x-api-key", api_key);
    }
    let response = request.send().await;
    response.assert_ok();
    response_message(response)
}

#[r2e::test(app = McpApp)]
async fn method_level_http_guard_gates_the_tool(app: TestApp) {
    // `clear_log` carries `#[guard(ApiKeyGuard)]`: the guard reads the HTTP
    // headers of the transport request carrying the tool call, so the same
    // guard type protects an MCP tool exactly like an HTTP route.
    // Over raw HTTP on purpose: the guard's input is a transport header.
    let session = initialize(&app).await;

    // Missing / wrong key → the guard's 403 surfaces as a JSON-RPC error,
    // and the log is left untouched.
    let denied = call_clear_log(&app, &session, None).await;
    assert_eq!(denied["error"]["code"], -32600, "{denied}");
    assert_eq!(denied["error"]["data"], "forbidden", "{denied}");
    let denied = call_clear_log(&app, &session, Some("wrong")).await;
    assert_eq!(denied["error"]["data"], "forbidden", "{denied}");

    // Right key → the tool runs.
    let cleared = call_clear_log(&app, &session, Some("letmein")).await;
    assert!(cleared.get("error").is_none(), "{cleared}");
    let text = cleared["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("cleared"), "{cleared}");
}

#[r2e::test(app = McpApp)]
async fn toolsets_are_per_session(app: TestApp) {
    let a = app.mcp().await;
    let b = app.mcp().await;
    assert!(!a.tool_names().await.contains(&"sqrt".to_string()));

    a.call_tool_text("enable_scientific", ()).await.unwrap();
    // The server tells session A — and only A — that its tool list moved.
    a.wait_for(|n| *n == McpNotification::ToolListChanged).await;
    assert!(a.tool_names().await.contains(&"sqrt".to_string()));
    let root: Calc = a.call_tool("sqrt", json!({ "x": 9.0 })).await.unwrap();
    assert_eq!(root.value, 3.0);

    // Session B never enabled the group: `sqrt` is unknown there.
    let hidden = b
        .call_tool::<Calc>("sqrt", json!({ "x": 9.0 }))
        .await
        .err()
        .expect("sqrt must be hidden from session B");
    assert_eq!(hidden.code(), Some(-32601), "{hidden}");
    assert!(b.notifications().is_empty());

    let name = b
        .call_tool_text("remember", json!({ "name": "half", "value": 0.5 }))
        .await
        .unwrap();
    assert_eq!(name, "const_half");
    assert_eq!(b.call_tool_text("const_half", ()).await.unwrap(), "0.5");
    assert!(!a.tool_names().await.contains(&"const_half".to_string()));
}
