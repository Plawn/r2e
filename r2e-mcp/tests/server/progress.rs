//! `Progress` member parameters on the wire: `notifications/progress` rides
//! the request's own SSE stream before the result, only when the client
//! sent a `progressToken`, strictly increasing — and survives
//! `mcp.json-response` (rmcp falls back to SSE).

use r2e_core::http::{Router, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{AppBuilderMcpExt, McpServer, Progress, ResourceCall, ToolCall};
use serde_json::{json, Value};

use crate::support::{self, McpResponse};

#[controller]
pub struct ProgressTools;

#[mcp_routes]
impl ProgressTools {
    /// Reports 1, 2, 2 (dropped: not increasing), 1.5 (dropped), 3 of 3.
    #[tool]
    async fn crunch(&self, progress: Progress) -> String {
        for (step, message) in [
            (1.0, Some("one")),
            (2.0, None),
            (2.0, None),
            (1.5, None),
            (3.0, Some("done")),
        ] {
            progress.report(step, Some(3.0), message).await;
        }
        format!("requested={}", progress.is_requested())
    }

    /// `Progress` alongside the raw call (the saved-field codegen path).
    #[tool]
    async fn crunch_with_call(&self, call: ToolCall, progress: Progress) -> String {
        progress.report(1.0, None, None).await;
        format!("id={}", call.request_id)
    }

    /// Resource reads report progress too.
    #[resource(uri = "progress://report")]
    async fn report(&self, progress: Progress, call: ResourceCall) -> String {
        progress.report(0.5, Some(1.0), None).await;
        call.progress.report(1.0, Some(1.0), None).await;
        "report".to_string()
    }

    /// So do prompt expansions.
    #[prompt]
    async fn slow_prompt(&self, progress: Progress) -> String {
        progress.report(1.0, None, Some("thinking")).await;
        "prompt".to_string()
    }
}

async fn app(plugin: McpServer) -> Router {
    AppBuilder::new()
        .plugin(plugin)
        .build_state()
        .await
        .register_mcp_service::<ProgressTools>()
        .build()
}

fn request(method: &str, params: Value, token: Option<Value>) -> Value {
    let mut params = params;
    if let Some(token) = token {
        params["_meta"] = json!({ "progressToken": token });
    }
    json!({ "jsonrpc": "2.0", "id": 9, "method": method, "params": params })
}

/// The `notifications/progress` params of a response, in stream order.
fn progress_of(response: &McpResponse) -> Vec<Value> {
    response
        .messages
        .iter()
        .filter(|m| m["method"] == "notifications/progress")
        .map(|m| m["params"].clone())
        .collect()
}

/// The final JSON-RPC response — asserted to be the last message.
fn final_result(response: &McpResponse) -> &Value {
    let last = response.messages.last().expect("no JSON-RPC message");
    assert_eq!(last["id"], 9, "the result must close the stream: {last}");
    assert!(last.get("error").is_none(), "{last}");
    &last["result"]
}

#[r2e_core::test]
async fn token_present_streams_increasing_reports_before_the_result() {
    let router = app(McpServer::new()).await;
    let session = support::initialize(&router, "/mcp").await;
    let response = support::post(
        &router,
        "/mcp",
        Some(&session),
        &request(
            "tools/call",
            json!({ "name": "crunch", "arguments": {} }),
            Some(json!("tok-1")),
        ),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);

    let reports = progress_of(&response);
    assert_eq!(
        reports,
        vec![
            json!({ "progressToken": "tok-1", "progress": 1.0, "total": 3.0, "message": "one" }),
            json!({ "progressToken": "tok-1", "progress": 2.0, "total": 3.0 }),
            json!({ "progressToken": "tok-1", "progress": 3.0, "total": 3.0, "message": "done" }),
        ],
        "non-increasing reports are dropped, the rest arrive in order"
    );
    assert_eq!(
        final_result(&response)["content"][0]["text"],
        "requested=true"
    );
}

#[r2e_core::test]
async fn no_token_sends_nothing() {
    let router = app(McpServer::new()).await;
    let session = support::initialize(&router, "/mcp").await;
    let response = support::post(
        &router,
        "/mcp",
        Some(&session),
        &request(
            "tools/call",
            json!({ "name": "crunch", "arguments": {} }),
            None,
        ),
    )
    .await;
    assert!(progress_of(&response).is_empty(), "{}", response.raw_body);
    assert_eq!(response.result()["content"][0]["text"], "requested=false");
}

#[r2e_core::test]
async fn numeric_token_and_raw_call_alongside() {
    let router = app(McpServer::new()).await;
    let session = support::initialize(&router, "/mcp").await;
    let response = support::post(
        &router,
        "/mcp",
        Some(&session),
        &request(
            "tools/call",
            json!({ "name": "crunch_with_call", "arguments": {} }),
            Some(json!(42)),
        ),
    )
    .await;
    assert_eq!(
        progress_of(&response),
        vec![json!({ "progressToken": 42, "progress": 1.0 })]
    );
    assert_eq!(final_result(&response)["content"][0]["text"], "id=9");
}

#[r2e_core::test]
async fn resources_and_prompts_report_progress() {
    let router = app(McpServer::new()).await;
    let session = support::initialize(&router, "/mcp").await;

    let read = support::post(
        &router,
        "/mcp",
        Some(&session),
        &request(
            "resources/read",
            json!({ "uri": "progress://report" }),
            Some(json!("r")),
        ),
    )
    .await;
    // The parameter and `ResourceCall::progress` share one reporter: the
    // monotonic check spans both.
    assert_eq!(
        progress_of(&read),
        vec![
            json!({ "progressToken": "r", "progress": 0.5, "total": 1.0 }),
            json!({ "progressToken": "r", "progress": 1.0, "total": 1.0 }),
        ]
    );
    assert_eq!(final_result(&read)["contents"][0]["text"], "report");

    let get = support::post(
        &router,
        "/mcp",
        Some(&session),
        &request(
            "prompts/get",
            json!({ "name": "slow_prompt" }),
            Some(json!("p")),
        ),
    )
    .await;
    assert_eq!(
        progress_of(&get),
        vec![json!({ "progressToken": "p", "progress": 1.0, "message": "thinking" })]
    );
    final_result(&get);
}

#[r2e_core::test]
async fn json_response_mode_falls_back_to_sse_for_progress() {
    let router = app(McpServer::new().stateless(true).json_response(true)).await;

    // Without a token the reply stays plain JSON…
    let plain = support::post(
        &router,
        "/mcp",
        None,
        &request(
            "tools/call",
            json!({ "name": "crunch", "arguments": {} }),
            None,
        ),
    )
    .await;
    assert!(
        plain.content_type.starts_with("application/json"),
        "{}",
        plain.content_type
    );
    assert_eq!(plain.result()["content"][0]["text"], "requested=false");

    // …with one, rmcp switches to SSE so no report is lost.
    let streamed = support::post(
        &router,
        "/mcp",
        None,
        &request(
            "tools/call",
            json!({ "name": "crunch", "arguments": {} }),
            Some(json!("j")),
        ),
    )
    .await;
    assert!(
        streamed.content_type.starts_with("text/event-stream"),
        "{}",
        streamed.content_type
    );
    assert_eq!(progress_of(&streamed).len(), 3, "{}", streamed.raw_body);
    assert_eq!(
        final_result(&streamed)["content"][0]["text"],
        "requested=true"
    );
}
