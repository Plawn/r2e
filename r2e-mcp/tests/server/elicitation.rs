//! `McpClient` member parameters on the wire: `elicitation/create` rides the
//! call's own SSE stream, the client answers on a separate POST routed by
//! `Mcp-Session-Id`, and the member resumes with the user's answer — or a
//! typed `ElicitError` (no capability, no session, timeout, cancellation,
//! bad answer, bad form type).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use http_body_util::BodyExt;
use r2e_core::http::{Body, Request, Router, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{AppBuilderMcpExt, ElicitError, Elicited, McpClient, McpError, McpServer, ToolCall};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::support;

#[derive(Deserialize, JsonSchema)]
struct Confirm {
    confirmed: bool,
    reason: Option<String>,
}

/// Not a valid elicitation form: properties must be primitives.
#[allow(dead_code)]
#[derive(Deserialize, JsonSchema)]
struct Nested {
    inner: Confirm,
}

static SAW_CANCELLED: AtomicBool = AtomicBool::new(false);

#[controller]
pub struct AskTools;

#[mcp_routes]
impl AskTools {
    #[tool]
    async fn confirm(&self, client: McpClient<'_>) -> Result<String, McpError> {
        Ok(match client.elicit::<Confirm>("Really?").await? {
            Elicited::Accept(c) => {
                format!("accept:{}:{}", c.confirmed, c.reason.unwrap_or_default())
            }
            Elicited::Decline => "decline".into(),
            Elicited::Cancel => "cancel".into(),
        })
    }

    /// Reports the raw `ElicitError` instead of `?`.
    #[tool]
    async fn probe(&self, client: McpClient<'_>) -> String {
        let supported = client.supports_elicitation();
        match client.elicit::<Confirm>("Probe").await {
            Ok(_) => format!("supported={supported} answered"),
            Err(err) => format!("supported={supported} err={err:?}"),
        }
    }

    #[tool]
    async fn nested(&self, client: McpClient<'_>) -> Result<String, McpError> {
        client.elicit::<Nested>("Nested").await?;
        Ok("unreachable".into())
    }

    #[tool]
    async fn watch_cancel(&self, client: McpClient<'_>) -> String {
        if let Err(ElicitError::Cancelled) = client.elicit::<Confirm>("Wait").await {
            SAW_CANCELLED.store(true, Ordering::SeqCst);
        }
        "done".into()
    }

    /// `McpClient` alongside the raw call (the saved-channel codegen path),
    /// and URL mode.
    #[tool]
    async fn consent(&self, call: ToolCall, client: McpClient<'_>) -> Result<String, McpError> {
        let answer = client
            .elicit_url(
                "Grant access",
                "https://auth.example.com/consent",
                "consent-1",
            )
            .await?;
        Ok(format!("{answer:?} id={}", call.request_id))
    }

    #[resource(uri = "ask://caps")]
    async fn caps(&self, client: McpClient<'_>) -> String {
        format!(
            "form={} url={}",
            client.supports_elicitation(),
            client.supports_url_elicitation()
        )
    }

    #[prompt]
    async fn ask_prompt(&self, client: McpClient<'_>) -> Result<String, McpError> {
        match client.elicit::<Confirm>("Prompt?").await? {
            Elicited::Accept(c) => Ok(format!("prompt:{}", c.confirmed)),
            _ => Ok("prompt:none".into()),
        }
    }
}

async fn app(plugin: McpServer) -> Router {
    AppBuilder::new()
        .plugin(plugin)
        .build_state()
        .await
        .register_mcp_service::<AskTools>()
        .build()
}

/// Session handshake advertising `capabilities`.
async fn initialize(router: &Router, capabilities: Value) -> String {
    let mut body = support::initialize_body();
    body["params"]["capabilities"] = capabilities;
    let response = support::post(router, "/mcp", None, &body).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    let session = response.session_id.expect("no Mcp-Session-Id");
    let notified = support::post(
        router,
        "/mcp",
        Some(&session),
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    assert_eq!(notified.status, StatusCode::ACCEPTED);
    session
}

/// An in-flight POST whose SSE body is read message by message, so the test
/// can answer a server→client request before the call completes.
struct Stream {
    body: Body,
    buf: String,
    queue: VecDeque<Value>,
}

impl Stream {
    async fn open(router: &Router, session: Option<&str>, body: &Value) -> Stream {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(sid) = session {
            builder = builder.header("mcp-session-id", sid);
        }
        let request = builder.body(Body::from(body.to_string())).unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        Stream {
            body: response.into_body(),
            buf: String::new(),
            queue: VecDeque::new(),
        }
    }

    /// The next JSON-RPC message, or `None` once the stream ends.
    async fn next(&mut self) -> Option<Value> {
        loop {
            if let Some(message) = self.queue.pop_front() {
                return Some(message);
            }
            let frame = tokio::time::timeout(Duration::from_secs(10), self.body.frame())
                .await
                .expect("SSE stream stalled")?
                .unwrap();
            let Ok(data) = frame.into_data() else {
                continue;
            };
            self.buf.push_str(&String::from_utf8_lossy(&data));
            while let Some(end) = self.buf.find("\n\n") {
                let event: String = self.buf.drain(..end + 2).collect();
                for line in event.lines() {
                    if let Some(data) = line.strip_prefix("data:").map(str::trim) {
                        if !data.is_empty() {
                            self.queue.push_back(serde_json::from_str(data).unwrap());
                        }
                    }
                }
            }
        }
    }

    /// The next message, asserted to be an `elicitation/create` request.
    async fn elicitation(&mut self) -> Value {
        let message = self.next().await.expect("stream ended before elicitation");
        assert_eq!(message["method"], "elicitation/create", "{message}");
        message
    }

    /// The final response, asserted to be the last message of the stream.
    async fn response(&mut self) -> Value {
        let message = self.next().await.expect("stream ended without a response");
        assert_eq!(message["id"], 9, "{message}");
        assert!(self.next().await.is_none(), "messages after the response");
        message
    }
}

fn call(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": 9, "method": method, "params": params })
}

fn tool(name: &str) -> Value {
    call("tools/call", json!({ "name": name, "arguments": {} }))
}

async fn answer(router: &Router, session: &str, request: &Value, result: Value) {
    let posted = support::post(
        router,
        "/mcp",
        Some(session),
        &json!({ "jsonrpc": "2.0", "id": request["id"], "result": result }),
    )
    .await;
    assert_eq!(posted.status, StatusCode::ACCEPTED, "{}", posted.raw_body);
}

fn text(response: &Value) -> &str {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("{response}"))
}

#[r2e_core::test]
async fn accept_decline_cancel_reach_the_member() {
    let router = app(McpServer::new()).await;
    let session = initialize(&router, json!({ "elicitation": {} })).await;

    let mut stream = Stream::open(&router, Some(&session), &tool("confirm")).await;
    let request = stream.elicitation().await;
    let params = &request["params"];
    assert_eq!(params["mode"], "form");
    assert_eq!(params["message"], "Really?");
    assert_eq!(params["requestedSchema"]["type"], "object");
    assert_eq!(
        params["requestedSchema"]["properties"]["confirmed"]["type"],
        "boolean"
    );
    answer(
        &router,
        &session,
        &request,
        json!({ "action": "accept", "content": { "confirmed": true, "reason": "sure" } }),
    )
    .await;
    assert_eq!(text(&stream.response().await), "accept:true:sure");

    for (action, expected) in [("decline", "decline"), ("cancel", "cancel")] {
        let mut stream = Stream::open(&router, Some(&session), &tool("confirm")).await;
        let request = stream.elicitation().await;
        answer(&router, &session, &request, json!({ "action": action })).await;
        assert_eq!(text(&stream.response().await), expected);
    }
}

#[r2e_core::test]
async fn an_answer_not_matching_the_type_is_a_tool_error() {
    let router = app(McpServer::new()).await;
    let session = initialize(&router, json!({ "elicitation": {} })).await;

    let mut stream = Stream::open(&router, Some(&session), &tool("confirm")).await;
    let request = stream.elicitation().await;
    answer(
        &router,
        &session,
        &request,
        json!({ "action": "accept", "content": { "confirmed": "yes" } }),
    )
    .await;
    let response = stream.response().await;
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert!(
        text(&response).starts_with("invalid elicitation answer"),
        "{response}"
    );
}

#[r2e_core::test]
async fn a_client_without_the_capability_is_never_asked() {
    let router = app(McpServer::new()).await;
    let session = initialize(&router, json!({})).await;
    let response = support::tools_call(&router, "/mcp", &session, "probe", json!({})).await;
    assert_eq!(
        response["result"]["content"][0]["text"],
        "supported=false err=Unsupported"
    );
}

#[r2e_core::test]
async fn sessionless_calls_fail_fast_with_no_channel() {
    let router = app(McpServer::new().stateless(true)).await;
    let response = support::post(&router, "/mcp", None, &tool("probe")).await;
    assert_eq!(
        response.result()["content"][0]["text"],
        "supported=false err=NoChannel"
    );
}

#[r2e_core::test]
async fn an_unanswered_elicitation_times_out() {
    let router = app(McpServer::new().with_elicitation_timeout(Duration::from_millis(200))).await;
    let session = initialize(&router, json!({ "elicitation": {} })).await;
    let mut stream = Stream::open(&router, Some(&session), &tool("probe")).await;
    stream.elicitation().await;
    assert_eq!(text(&stream.response().await), "supported=true err=Timeout");
}

#[r2e_core::test]
async fn cancelling_the_call_stops_the_wait() {
    let router = app(McpServer::new()).await;
    let session = initialize(&router, json!({ "elicitation": {} })).await;
    let mut stream = Stream::open(&router, Some(&session), &tool("watch_cancel")).await;
    stream.elicitation().await;
    let posted = support::post(
        &router,
        "/mcp",
        Some(&session),
        &json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": { "requestId": 9 }
        }),
    )
    .await;
    assert_eq!(posted.status, StatusCode::ACCEPTED);
    for _ in 0..100 {
        if SAW_CANCELLED.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the member never observed ElicitError::Cancelled");
}

#[r2e_core::test]
async fn a_non_flat_form_type_fails_before_anything_is_sent() {
    let router = app(McpServer::new()).await;
    let session = initialize(&router, json!({ "elicitation": {} })).await;
    let mut stream = Stream::open(&router, Some(&session), &tool("nested")).await;
    let response = stream.response().await;
    assert_eq!(response["error"]["code"], -32603, "{response}");
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not a flat object"),
        "{response}"
    );
}

#[r2e_core::test]
async fn url_mode_alongside_the_raw_call() {
    let router = app(McpServer::new()).await;
    let session = initialize(&router, json!({ "elicitation": { "url": {} } })).await;
    let mut stream = Stream::open(&router, Some(&session), &tool("consent")).await;
    let request = stream.elicitation().await;
    assert_eq!(request["params"]["mode"], "url");
    assert_eq!(request["params"]["url"], "https://auth.example.com/consent");
    assert_eq!(request["params"]["elicitationId"], "consent-1");
    answer(&router, &session, &request, json!({ "action": "accept" })).await;
    assert_eq!(text(&stream.response().await), "Accept(()) id=9");
}

#[r2e_core::test]
async fn url_only_clients_do_not_get_forms() {
    let router = app(McpServer::new()).await;
    let session = initialize(&router, json!({ "elicitation": { "url": {} } })).await;
    let response = support::tools_call(&router, "/mcp", &session, "probe", json!({})).await;
    assert_eq!(
        response["result"]["content"][0]["text"],
        "supported=false err=Unsupported"
    );
}

#[r2e_core::test]
async fn resources_and_prompts_take_a_client() {
    let router = app(McpServer::new()).await;
    let session = initialize(&router, json!({ "elicitation": { "form": {} } })).await;

    let read = support::post(
        &router,
        "/mcp",
        Some(&session),
        &call("resources/read", json!({ "uri": "ask://caps" })),
    )
    .await;
    assert_eq!(read.result()["contents"][0]["text"], "form=true url=false");

    let mut stream = Stream::open(
        &router,
        Some(&session),
        &call("prompts/get", json!({ "name": "ask_prompt" })),
    )
    .await;
    let request = stream.elicitation().await;
    answer(
        &router,
        &session,
        &request,
        json!({ "action": "accept", "content": { "confirmed": false } }),
    )
    .await;
    let response = stream.response().await;
    assert_eq!(
        response["result"]["messages"][0]["content"]["text"], "prompt:false",
        "{response}"
    );
}

#[r2e_core::test]
async fn hand_built_calls_have_no_channel() {
    let call = ToolCall::new(json!({}));
    let client = call.client();
    assert!(!client.supports_elicitation());
    assert!(matches!(
        client.elicit::<Confirm>("x").await,
        Err(ElicitError::NoChannel)
    ));
    let err: McpError = ElicitError::NoChannel.into();
    assert!(matches!(err, McpError::Tool { .. }));
    let err: McpError = ElicitError::InvalidSchema("x".into()).into();
    assert!(matches!(err, McpError::Internal(_)));
}

/// A 2026-07-28 client negotiates per request, with no session: rmcp serves
/// it over a one-shot transport that drops the answer POST, so live
/// elicitation would hang — it fails fast instead (MRTR is the 2026 path).
#[r2e_core::test]
async fn per_request_2026_clients_fail_fast_with_no_channel() {
    let router = app(McpServer::new()).await;
    let mut body = tool("probe");
    body["params"]["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": { "name": "t", "version": "0" },
        "io.modelcontextprotocol/clientCapabilities": { "elicitation": {} },
    });
    let response = support::post_with_headers(
        &router,
        "/mcp",
        None,
        &[
            ("mcp-protocol-version", "2026-07-28"),
            ("mcp-method", "tools/call"),
            ("mcp-name", "probe"),
        ],
        &body,
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    assert_eq!(
        response.result()["content"][0]["text"],
        "supported=false err=NoChannel"
    );
}
