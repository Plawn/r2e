//! Shared helpers for r2e-mcp integration tests.
//!
//! Drives the MCP streamable-HTTP endpoint through
//! `tower::ServiceExt::oneshot` on the built app router — no live TCP. The
//! JSON-RPC shapes and the session dance (initialize → `Mcp-Session-Id`
//! response header → `notifications/initialized` → requests) mirror what a
//! real client sends; responses arrive as SSE (`data:` lines) or plain JSON
//! (`mcp.json-response` mode) and both are parsed here.

use http_body_util::BodyExt;
use r2e_core::http::{Body, Request, Router, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// A parsed response from the MCP endpoint.
pub struct McpResponse {
    pub status: StatusCode,
    pub content_type: String,
    pub session_id: Option<String>,
    /// The JSON-RPC messages carried in the body: each SSE `data:` event, or
    /// the single JSON body, or empty (202 on notifications).
    pub messages: Vec<Value>,
    pub raw_body: String,
}

impl McpResponse {
    /// The single JSON-RPC message of a request/response exchange.
    pub fn message(&self) -> &Value {
        assert_eq!(
            self.messages.len(),
            1,
            "expected exactly one JSON-RPC message, got {}: {}",
            self.messages.len(),
            self.raw_body
        );
        &self.messages[0]
    }

    /// The `result` of a successful JSON-RPC response.
    pub fn result(&self) -> &Value {
        let msg = self.message();
        assert!(
            msg.get("error").is_none(),
            "JSON-RPC error where a result was expected: {msg}"
        );
        &msg["result"]
    }
}

/// POST a JSON-RPC body to the MCP endpoint with extra request headers.
pub async fn post_with_headers(
    router: &Router,
    path: &str,
    session: Option<&str>,
    headers: &[(&str, &str)],
    body: &Value,
) -> McpResponse {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        // rmcp's DNS-rebinding protection allows loopback hosts by default.
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(sid) = session {
        builder = builder.header("mcp-session-id", sid);
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(Body::from(body.to_string())).unwrap();
    let response = router.clone().oneshot(request).await.unwrap();

    let (parts, body) = response.into_parts();
    let session_id = parts
        .headers
        .get("mcp-session-id")
        .map(|v| v.to_str().unwrap().to_string());
    let content_type = parts
        .headers
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let bytes = body.collect().await.unwrap().to_bytes();
    let raw_body = String::from_utf8_lossy(&bytes).into_owned();

    let messages = if content_type.starts_with("text/event-stream") {
        raw_body
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            // rmcp primes each stream with an empty `data:` event (id/retry
            // bookkeeping) — only non-empty payloads are JSON-RPC messages.
            .filter(|data| !data.is_empty())
            .map(|data| {
                serde_json::from_str(data)
                    .unwrap_or_else(|e| panic!("SSE data event is not JSON ({e}): {data:?}"))
            })
            .collect()
    } else if content_type.starts_with("application/json") && !raw_body.trim().is_empty() {
        vec![serde_json::from_str(&raw_body)
            .unwrap_or_else(|e| panic!("body is not JSON ({e}): {raw_body}"))]
    } else {
        // Empty bodies (202 on notifications) and plain-text transport
        // rejections carry no JSON-RPC message.
        Vec::new()
    };

    McpResponse {
        status: parts.status,
        content_type,
        session_id,
        messages,
        raw_body,
    }
}

/// POST a JSON-RPC body to the MCP endpoint.
pub async fn post(
    router: &Router,
    path: &str,
    session: Option<&str>,
    body: &Value,
) -> McpResponse {
    post_with_headers(router, path, session, &[], body).await
}

pub fn initialize_body() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "r2e-mcp-tests", "version": "0.0.0" }
        }
    })
}

/// Full session handshake: initialize → capture `Mcp-Session-Id` →
/// `notifications/initialized`. Returns the session id.
pub async fn initialize(router: &Router, path: &str) -> String {
    let response = post(router, path, None, &initialize_body()).await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "initialize failed: {}",
        response.raw_body
    );
    let session = response
        .session_id
        .clone()
        .expect("initialize response carries no Mcp-Session-Id header");
    let notified = post(
        router,
        path,
        Some(&session),
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    assert_eq!(notified.status, StatusCode::ACCEPTED);
    session
}

/// `tools/list` on an initialized session; returns the `result` object.
pub async fn tools_list(router: &Router, path: &str, session: &str) -> Value {
    let response = post(
        router,
        path,
        Some(session),
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    response.result().clone()
}

/// `tools/call` on an initialized session; returns the full JSON-RPC message
/// (so callers can assert on either `result` or `error`).
pub async fn tools_call(
    router: &Router,
    path: &str,
    session: &str,
    name: &str,
    arguments: Value,
) -> Value {
    tools_call_with_headers(router, path, session, &[], name, arguments).await
}

/// `tools/call` with extra HTTP headers on the carrying request (guards see
/// the real transport headers).
pub async fn tools_call_with_headers(
    router: &Router,
    path: &str,
    session: &str,
    headers: &[(&str, &str)],
    name: &str,
    arguments: Value,
) -> Value {
    let response = post_with_headers(
        router,
        path,
        Some(session),
        headers,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    response.message().clone()
}

/// Find one tool by name in a `tools/list` result.
pub fn tool<'a>(list_result: &'a Value, name: &str) -> &'a Value {
    list_result["tools"]
        .as_array()
        .expect("tools/list result has no tools array")
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("tool `{name}` not listed in {list_result}"))
}
