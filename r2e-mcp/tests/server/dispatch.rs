//! `tools/call` dispatch: happy paths, error mapping (domain vs protocol),
//! guards over real transport headers, and the `ToolCall` context.

use serde_json::json;

use crate::fixtures::fixture_app;
use crate::support::{initialize, tools_call, tools_call_with_headers};

#[r2e_core::test]
async fn call_with_json_return_is_dual_encoded() {
    let (router, _log) = fixture_app().await;
    let session = initialize(&router, "/mcp").await;

    let msg = tools_call(&router, "/mcp", &session, "add", json!({"a": 2.0, "b": 3.0})).await;
    let result = &msg["result"];
    assert_ne!(result["isError"], true, "{msg}");
    // `Json<T>` results are dual-encoded: structuredContent + a JSON text
    // content block carrying the same document.
    assert_eq!(result["structuredContent"]["value"], 5.0);
    let text = result["content"][0]["text"].as_str().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["value"], 5.0);
}

#[r2e_core::test]
async fn domain_error_is_an_is_error_result() {
    let (router, _log) = fixture_app().await;
    let session = initialize(&router, "/mcp").await;

    // `McpError::tool` → CallToolResult{isError:true}: readable by the
    // calling agent, NOT a protocol error.
    let msg = tools_call(&router, "/mcp", &session, "div", json!({"a": 1.0, "b": 0.0})).await;
    let result = &msg["result"];
    assert_eq!(result["isError"], true, "{msg}");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("division by zero"), "{text}");
}

#[r2e_core::test]
async fn invalid_arguments_are_a_protocol_error() {
    let (router, _log) = fixture_app().await;
    let session = initialize(&router, "/mcp").await;

    // Wrong type.
    let msg = tools_call(&router, "/mcp", &session, "add", json!({"a": "x", "b": 3.0})).await;
    assert_eq!(msg["error"]["code"], -32602, "{msg}");

    // Missing required field.
    let msg = tools_call(&router, "/mcp", &session, "add", json!({})).await;
    assert_eq!(msg["error"]["code"], -32602, "{msg}");
}

#[r2e_core::test]
async fn unknown_tool_is_method_not_found() {
    let (router, _log) = fixture_app().await;
    let session = initialize(&router, "/mcp").await;

    let msg = tools_call(&router, "/mcp", &session, "nope", json!({})).await;
    assert_eq!(msg["error"]["code"], -32601, "{msg}");
    assert!(
        msg["error"]["message"].as_str().unwrap().contains("nope"),
        "{msg}"
    );
}

#[r2e_core::test]
async fn tool_call_context_exposes_the_request_id() {
    let (router, _log) = fixture_app().await;
    let session = initialize(&router, "/mcp").await;

    // support::tools_call sends JSON-RPC id 3.
    let msg = tools_call(&router, "/mcp", &session, "echo_id", json!({})).await;
    assert_eq!(msg["result"]["content"][0]["text"], "id=3", "{msg}");
}

#[r2e_core::test]
async fn guard_rejection_maps_to_invalid_request() {
    let (router, _log) = fixture_app().await;
    let session = initialize(&router, "/mcp").await;

    // Guards see the REAL transport headers; the 403 rejection is folded
    // into a JSON-RPC error with a machine-readable marker.
    let msg = tools_call(&router, "/mcp", &session, "locked", json!({})).await;
    assert_eq!(msg["error"]["code"], -32600, "{msg}");
    assert_eq!(msg["error"]["data"], "forbidden", "{msg}");
    assert!(
        msg["error"]["message"]
            .as_str()
            .unwrap()
            .contains("x-test-key"),
        "the HTTP rejection body is relayed as the message: {msg}"
    );
}

#[r2e_core::test]
async fn guard_passes_with_the_right_header() {
    let (router, _log) = fixture_app().await;
    let session = initialize(&router, "/mcp").await;

    let msg = tools_call_with_headers(
        &router,
        "/mcp",
        &session,
        &[("x-test-key", "sesame")],
        "locked",
        json!({}),
    )
    .await;
    assert_eq!(msg["result"]["content"][0]["text"], "unlocked", "{msg}");
}

#[r2e_core::test]
async fn request_without_session_is_rejected_in_stateful_mode() {
    let (router, _log) = fixture_app().await;
    let _session = initialize(&router, "/mcp").await;

    let response = crate::support::post(
        &router,
        "/mcp",
        None,
        &json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/list" }),
    )
    .await;
    assert_ne!(
        response.status.as_u16(),
        200,
        "a session-less request must not reach dispatch: {}",
        response.raw_body
    );
}
