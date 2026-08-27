//! End-to-end tests booting the REAL example-mcp blueprint (`McpApp`) via
//! `#[r2e::test(app = ...)]` and driving the MCP endpoint plus the HTTP
//! adapter over the same shared bean.

use example_mcp::McpApp;
use r2e_test::TestApp;
use serde_json::{json, Value};

// ── Minimal MCP client over TestApp ─────────────────────────────────────

/// Extract the JSON-RPC messages from an MCP endpoint response body
/// (SSE `data:` events, skipping rmcp's empty priming event).
fn sse_messages(body: &str) -> Vec<Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|data| !data.is_empty())
        .map(|data| serde_json::from_str(data).expect("SSE data event is not JSON"))
        .collect()
}

async fn mcp_post(app: &TestApp, session: Option<&str>, body: &Value) -> r2e_test::TestResponse {
    let mut request = app
        .post("/mcp")
        .header("host", "localhost")
        .header("accept", "application/json, text/event-stream")
        .json(body);
    if let Some(sid) = session {
        request = request.header("mcp-session-id", sid);
    }
    request.send().await
}

/// initialize → capture `Mcp-Session-Id` → notifications/initialized.
async fn initialize(app: &TestApp) -> String {
    let response = mcp_post(
        app,
        None,
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "example-mcp-e2e", "version": "0.0.0" }
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

async fn tools_call(app: &TestApp, session: &str, name: &str, arguments: Value) -> Value {
    tools_call_with(app, session, &[], name, arguments).await
}

async fn tools_call_with(
    app: &TestApp,
    session: &str,
    headers: &[(&'static str, &'static str)],
    name: &str,
    arguments: Value,
) -> Value {
    let mut request = app
        .post("/mcp")
        .header("host", "localhost")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", session)
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

#[r2e::test(app = McpApp)]
async fn tools_are_listed_with_schemas(app: TestApp) {
    let session = initialize(&app).await;
    let response = mcp_post(
        &app,
        Some(&session),
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    response.assert_ok();
    let messages = sse_messages(&response.text());
    let tools = messages[0]["result"]["tools"].as_array().unwrap();

    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["add", "call_log", "clear_log", "divide"]);

    let add = tools.iter().find(|t| t["name"] == "add").unwrap();
    assert_eq!(add["description"], "Add two numbers and return their sum.");
    assert_eq!(add["inputSchema"]["properties"]["a"]["description"], "Left operand.");
    assert_eq!(add["annotations"]["readOnlyHint"], true);
    assert!(add["outputSchema"]["properties"]["value"].is_object());
}

#[r2e::test(app = McpApp)]
async fn tool_call_returns_structured_content(app: TestApp) {
    let session = initialize(&app).await;
    let msg = tools_call(&app, &session, "add", json!({"a": 2.0, "b": 3.0})).await;
    assert_eq!(msg["result"]["structuredContent"]["value"], 5.0, "{msg}");
}

#[r2e::test(app = McpApp)]
async fn domain_error_is_agent_readable(app: TestApp) {
    let session = initialize(&app).await;
    let msg = tools_call(&app, &session, "divide", json!({"a": 1.0, "b": 0.0})).await;
    assert_eq!(msg["result"]["isError"], true, "{msg}");
    let text = msg["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("division by zero"), "{text}");
}

#[r2e::test(app = McpApp)]
async fn guarded_tool_requires_the_api_key(app: TestApp) {
    let session = initialize(&app).await;

    let denied = tools_call(&app, &session, "clear_log", json!({})).await;
    assert_eq!(denied["error"]["code"], -32600, "{denied}");
    assert_eq!(denied["error"]["data"], "forbidden", "{denied}");

    let allowed = tools_call_with(
        &app,
        &session,
        &[("x-api-key", "letmein")],
        "clear_log",
        json!({}),
    )
    .await;
    let text = allowed["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("cleared "), "{text}");
}

#[r2e::test(app = McpApp)]
async fn interceptor_logs_are_visible_through_the_call_log_tool(app: TestApp) {
    let session = initialize(&app).await;
    tools_call(&app, &session, "divide", json!({"a": 6.0, "b": 2.0})).await;

    let msg = tools_call(&app, &session, "call_log", json!({})).await;
    assert_eq!(
        msg["result"]["content"][0]["text"], "tool:divide",
        "the graph-built LogCalls interceptor writes to the same CallLog bean: {msg}"
    );
}

#[r2e::test(app = McpApp)]
async fn http_adapter_shares_the_same_service(app: TestApp) {
    // The HTTP controller and the MCP service are thin adapters over ONE
    // CalcService bean.
    let response = app.get("/api/calc/add/2/3").send().await;
    response.assert_ok();
    let body: Value = response.json();
    assert_eq!(body["value"], 5.0);
}
