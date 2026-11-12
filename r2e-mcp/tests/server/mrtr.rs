//! Multi-round-trip requests (protocol 2026-07-28): a sessionless client has
//! no channel for `elicitation/create`, so `McpClient::elicit` answers the
//! call with `input_required` and the member runs again once the client
//! retries with its answers and the echoed `requestState`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use r2e_core::http::{Routes, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{AppBuilderMcpExt, Elicited, McpClient, McpError, McpServer, Params};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::support::{self, McpResponse};

const KEY: &str = "0123456789abcdef0123456789abcdef";

#[derive(Deserialize, JsonSchema)]
struct Confirm {
    confirmed: bool,
}

#[derive(Deserialize, JsonSchema, ObjectParams)]
struct Target {
    id: u32,
    #[serde(default)]
    force: bool,
}

/// Runs of `counted`, to observe that the member restarts from the top.
static COUNTED_RUNS: AtomicUsize = AtomicUsize::new(0);

#[controller]
pub struct RoundTools;

#[routes]
impl RoundTools {
    #[tool]
    async fn delete(
        &self,
        Params(target): Params<Target>,
        client: McpClient<'_>,
    ) -> Result<String, McpError> {
        Ok(
            match client.elicit::<Confirm>("confirm", "Delete it?").await? {
                Elicited::Accept(c) => format!("deleted:{}:{}:{}", target.id, target.force, c.confirmed),
                Elicited::Decline => "declined".into(),
                Elicited::Cancel => "cancelled".into(),
            },
        )
    }

    #[tool]
    async fn counted(&self, client: McpClient<'_>) -> Result<String, McpError> {
        let run = COUNTED_RUNS.fetch_add(1, Ordering::SeqCst) + 1;
        client.elicit::<Confirm>("go", "Go?").await?;
        Ok(format!("run:{run}"))
    }

    /// Asks both questions in the same round: nothing is `?`-ed before the
    /// second one is recorded.
    #[tool]
    async fn both(&self, client: McpClient<'_>) -> Result<String, McpError> {
        let first = client.elicit::<Confirm>("first", "First?").await;
        let second = client.elicit::<Confirm>("second", "Second?").await;
        match (first?, second?) {
            (Elicited::Accept(a), Elicited::Accept(b)) => {
                Ok(format!("both:{}:{}", a.confirmed, b.confirmed))
            }
            _ => Ok("both:refused".into()),
        }
    }

    /// One question per round, in sequence.
    #[tool]
    async fn sequence(&self, client: McpClient<'_>) -> Result<String, McpError> {
        let mut answers = Vec::new();
        for step in 0..3 {
            match client
                .elicit::<Confirm>(&format!("step-{step}"), "Next?")
                .await?
            {
                Elicited::Accept(c) => answers.push(c.confirmed.to_string()),
                _ => answers.push("-".into()),
            }
        }
        Ok(answers.join(","))
    }

    /// A member swallowing the error still yields `input_required`.
    #[tool]
    async fn swallow(&self, client: McpClient<'_>) -> String {
        match client.elicit::<Confirm>("q", "Sure?").await {
            Ok(Elicited::Accept(c)) => format!("answered:{}", c.confirmed),
            Ok(_) => "refused".into(),
            Err(err) => format!("err={err:?}"),
        }
    }

    #[tool]
    async fn caps(&self, client: McpClient<'_>) -> String {
        format!(
            "form={} url={}",
            client.supports_elicitation(),
            client.supports_url_elicitation()
        )
    }

    #[tool]
    async fn consent(&self, client: McpClient<'_>) -> Result<String, McpError> {
        let answer = client
            .elicit_url("Grant access", "https://auth.example.com/consent", "consent-1")
            .await?;
        Ok(format!("{answer:?}"))
    }

    #[resource(uri = "ask://secret")]
    async fn secret(&self, client: McpClient<'_>) -> Result<String, McpError> {
        match client.elicit::<Confirm>("reveal", "Reveal?").await? {
            Elicited::Accept(c) if c.confirmed => Ok("s3cret".into()),
            _ => Ok("hidden".into()),
        }
    }

    #[prompt]
    async fn ask_prompt(&self, client: McpClient<'_>) -> Result<String, McpError> {
        match client.elicit::<Confirm>("prompt", "Prompt?").await? {
            Elicited::Accept(c) => Ok(format!("prompt:{}", c.confirmed)),
            _ => Ok("prompt:none".into()),
        }
    }
}

async fn app(plugin: McpServer) -> Routes {
    AppBuilder::new()
        .plugin(plugin)
        .build_state()
        .await
        .register_mcp_service::<RoundTools>()
        .build()
}

async fn keyed() -> Routes {
    app(McpServer::new().with_request_state_key(KEY)).await
}

/// One sessionless 2026-07-28 request. `name` is the `Mcp-Name` header: the
/// tool or prompt name, the resource URI.
async fn send(
    router: &Routes,
    method: &str,
    name: &str,
    mut params: Value,
    capabilities: Value,
) -> McpResponse {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": { "name": "t", "version": "0" },
        "io.modelcontextprotocol/clientCapabilities": capabilities,
    });
    let response = support::post_with_headers(
        router,
        "/mcp",
        None,
        &[
            ("mcp-protocol-version", "2026-07-28"),
            ("mcp-method", method),
            ("mcp-name", name),
        ],
        &json!({ "jsonrpc": "2.0", "id": 9, "method": method, "params": params }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    response
}

async fn tool(router: &Routes, name: &str, params: Value) -> McpResponse {
    let mut params = params;
    params["name"] = json!(name);
    if params.get("arguments").is_none() {
        params["arguments"] = json!({});
    }
    send(router, "tools/call", name, params, json!({ "elicitation": {} })).await
}

fn accept(confirmed: bool) -> Value {
    json!({ "action": "accept", "content": { "confirmed": confirmed } })
}

/// The `requestState` of an `input_required` result asking exactly `keys`.
fn state(result: &Value, keys: &[&str]) -> String {
    assert_eq!(result["resultType"], "input_required", "{result}");
    let requests = result["inputRequests"]
        .as_object()
        .unwrap_or_else(|| panic!("{result}"));
    let mut asked: Vec<&str> = requests.keys().map(String::as_str).collect();
    asked.sort_unstable();
    assert_eq!(asked, keys, "{result}");
    for request in requests.values() {
        assert_eq!(request["method"], "elicitation/create", "{result}");
    }
    result["requestState"].as_str().expect("requestState").to_owned()
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("{result}"))
}

fn error(response: &McpResponse) -> &Value {
    let error = &response.message()["error"];
    assert!(error.is_object(), "{}", response.raw_body);
    error
}

#[r2e_core::test]
async fn a_question_is_answered_on_the_retry() {
    let router = keyed().await;
    let args = json!({ "arguments": { "id": 7 } });

    let first = tool(&router, "delete", args.clone()).await;
    let sealed = state(first.result(), &["confirm"]);
    let request = &first.result()["inputRequests"]["confirm"];
    assert_eq!(request["params"]["message"], "Delete it?", "{request}");
    assert_eq!(
        request["params"]["requestedSchema"]["properties"]["confirmed"]["type"],
        "boolean",
        "{request}"
    );

    let mut retry = args;
    retry["inputResponses"] = json!({ "confirm": accept(true) });
    retry["requestState"] = json!(sealed);
    let second = tool(&router, "delete", retry).await;
    assert_eq!(text(second.result()), "deleted:7:false:true");
}

#[r2e_core::test]
async fn the_member_runs_again_from_the_top() {
    let router = keyed().await;
    let sealed = state(tool(&router, "counted", json!({})).await.result(), &["go"]);
    let second = tool(
        &router,
        "counted",
        json!({ "inputResponses": { "go": accept(true) }, "requestState": sealed }),
    )
    .await;
    assert_eq!(text(second.result()), "run:2");
    assert_eq!(COUNTED_RUNS.load(Ordering::SeqCst), 2);
}

#[r2e_core::test]
async fn decline_and_cancel_reach_the_member() {
    let router = keyed().await;
    for (action, expected) in [("decline", "declined"), ("cancel", "cancelled")] {
        let args = json!({ "arguments": { "id": 1 } });
        let sealed = state(tool(&router, "delete", args.clone()).await.result(), &["confirm"]);
        let mut retry = args;
        retry["inputResponses"] = json!({ "confirm": { "action": action } });
        retry["requestState"] = json!(sealed);
        assert_eq!(text(tool(&router, "delete", retry).await.result()), expected);
    }
}

#[r2e_core::test]
async fn several_questions_fit_in_one_round() {
    let router = keyed().await;
    let first = tool(&router, "both", json!({})).await;
    let sealed = state(first.result(), &["first", "second"]);
    let second = tool(
        &router,
        "both",
        json!({
            "inputResponses": { "first": accept(true), "second": accept(false) },
            "requestState": sealed,
        }),
    )
    .await;
    assert_eq!(text(second.result()), "both:true:false");
}

#[r2e_core::test]
async fn a_partial_answer_asks_the_rest_again() {
    let router = keyed().await;
    let sealed = state(tool(&router, "both", json!({})).await.result(), &["first", "second"]);
    let again = tool(
        &router,
        "both",
        json!({ "inputResponses": { "first": accept(true) }, "requestState": sealed }),
    )
    .await;
    let sealed = state(again.result(), &["second"]);
    let done = tool(
        &router,
        "both",
        json!({ "inputResponses": { "second": accept(true) }, "requestState": sealed }),
    )
    .await;
    assert_eq!(text(done.result()), "both:true:true");
}

#[r2e_core::test]
async fn earlier_answers_travel_in_the_state() {
    let router = keyed().await;
    let mut sealed = state(tool(&router, "sequence", json!({})).await.result(), &["step-0"]);
    for (step, next) in [(0, Some("step-1")), (1, Some("step-2")), (2, None)] {
        let key = format!("step-{step}");
        let response = tool(
            &router,
            "sequence",
            json!({ "inputResponses": { key: accept(step != 1) }, "requestState": sealed }),
        )
        .await;
        match next {
            Some(next) => sealed = state(response.result(), &[next]),
            None => {
                assert_eq!(text(response.result()), "true,false,true");
                return;
            }
        }
    }
}

#[r2e_core::test]
async fn a_pending_question_wins_over_the_members_own_result() {
    let router = keyed().await;
    let first = tool(&router, "swallow", json!({})).await;
    let sealed = state(first.result(), &["q"]);
    let second = tool(
        &router,
        "swallow",
        json!({ "inputResponses": { "q": accept(true) }, "requestState": sealed }),
    )
    .await;
    assert_eq!(text(second.result()), "answered:true");
}

#[r2e_core::test]
async fn unsolicited_answers_are_ignored() {
    let router = keyed().await;
    // No state: nothing was asked, so nothing is an answer.
    let first = tool(
        &router,
        "delete",
        json!({ "arguments": { "id": 1 }, "inputResponses": { "confirm": accept(true) } }),
    )
    .await;
    state(first.result(), &["confirm"]);

    // With a state: only the keys it asked are read.
    let sealed = state(tool(&router, "both", json!({})).await.result(), &["first", "second"]);
    let sealed = state(
        tool(
            &router,
            "both",
            json!({ "inputResponses": { "first": accept(true) }, "requestState": sealed }),
        )
        .await
        .result(),
        &["second"],
    );
    // `first` is settled: a later value for it is not taken.
    let done = tool(
        &router,
        "both",
        json!({
            "inputResponses": { "first": accept(false), "second": accept(true) },
            "requestState": sealed,
        }),
    )
    .await;
    assert_eq!(text(done.result()), "both:true:true");
}

#[r2e_core::test]
async fn a_state_is_bound_to_the_member_and_its_arguments() {
    let router = keyed().await;
    let sealed = state(
        tool(&router, "delete", json!({ "arguments": { "id": 1, "force": true } }))
            .await
            .result(),
        &["confirm"],
    );
    let retry = |arguments: Value| {
        json!({
            "arguments": arguments,
            "inputResponses": { "confirm": accept(true) },
            "requestState": sealed,
        })
    };

    // Other arguments.
    let other = tool(&router, "delete", retry(json!({ "id": 2, "force": true }))).await;
    assert_eq!(error(&other)["code"], -32602, "{}", other.raw_body);
    assert!(
        error(&other)["message"].as_str().unwrap().contains("requestState"),
        "{}",
        other.raw_body
    );
    // Another member.
    let other = tool(
        &router,
        "swallow",
        json!({ "inputResponses": { "q": accept(true) }, "requestState": sealed }),
    )
    .await;
    assert_eq!(error(&other)["code"], -32602, "{}", other.raw_body);
    // Same arguments in another order: same call.
    let same = tool(&router, "delete", retry(json!({ "force": true, "id": 1 }))).await;
    assert_eq!(text(same.result()), "deleted:1:true:true");
}

#[r2e_core::test]
async fn a_tampered_or_foreign_state_is_rejected() {
    let router = keyed().await;
    let args = json!({ "arguments": { "id": 1 } });
    let sealed = state(tool(&router, "delete", args.clone()).await.result(), &["confirm"]);
    let retry = |sealed: String| {
        let mut retry = args.clone();
        retry["inputResponses"] = json!({ "confirm": accept(true) });
        retry["requestState"] = json!(sealed);
        retry
    };

    let mut tampered = sealed.clone().into_bytes();
    let middle = tampered.len() / 2;
    tampered[middle] = if tampered[middle] == b'A' { b'B' } else { b'A' };
    let response = tool(&router, "delete", retry(String::from_utf8(tampered).unwrap())).await;
    assert_eq!(error(&response)["code"], -32602, "{}", response.raw_body);

    let response = tool(&router, "delete", retry("garbage".into())).await;
    assert_eq!(error(&response)["code"], -32602, "{}", response.raw_body);

    // Sealed under another key: another deployment's state.
    let other = app(McpServer::new().with_request_state_key("another-key-another-key-another-key")).await;
    let response = tool(&other, "delete", retry(sealed)).await;
    assert_eq!(error(&response)["code"], -32602, "{}", response.raw_body);
}

#[r2e_core::test]
async fn a_state_expires_with_the_elicitation_timeout() {
    let router = app(
        McpServer::new()
            .with_request_state_key(KEY)
            .with_elicitation_timeout(Duration::from_secs(1)),
    )
    .await;
    let args = json!({ "arguments": { "id": 1 } });
    let sealed = state(tool(&router, "delete", args.clone()).await.result(), &["confirm"]);
    tokio::time::sleep(Duration::from_millis(2100)).await;
    let mut retry = args;
    retry["inputResponses"] = json!({ "confirm": accept(true) });
    retry["requestState"] = json!(sealed);
    let response = tool(&router, "delete", retry).await;
    assert_eq!(error(&response)["code"], -32602, "{}", response.raw_body);
}

#[r2e_core::test]
async fn rounds_are_capped() {
    let router = AppBuilder::new()
        .override_config(
            r2e_core::R2eConfig::from_yaml_str(&format!(
                "mcp:\n  request-state-key: {KEY}\n  mrtr-max-rounds: 2\n"
            ))
            .unwrap(),
        )
        .load_config::<()>()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<RoundTools>()
        .build();
    let sealed = state(tool(&router, "sequence", json!({})).await.result(), &["step-0"]);
    let sealed = state(
        tool(
            &router,
            "sequence",
            json!({ "inputResponses": { "step-0": accept(true) }, "requestState": sealed }),
        )
        .await
        .result(),
        &["step-1"],
    );
    let response = tool(
        &router,
        "sequence",
        json!({ "inputResponses": { "step-1": accept(true) }, "requestState": sealed }),
    )
    .await;
    assert_eq!(error(&response)["code"], -32600, "{}", response.raw_body);
    assert!(
        error(&response)["message"].as_str().unwrap().contains("mcp.mrtr-max-rounds"),
        "{}",
        response.raw_body
    );
}

#[r2e_core::test]
async fn a_bad_answer_is_an_error_not_a_new_round() {
    let router = keyed().await;
    let args = json!({ "arguments": { "id": 1 } });
    let sealed = state(tool(&router, "delete", args.clone()).await.result(), &["confirm"]);
    let mut retry = args;
    retry["inputResponses"] =
        json!({ "confirm": { "action": "accept", "content": { "confirmed": "maybe" } } });
    retry["requestState"] = json!(sealed);
    let response = tool(&router, "delete", retry).await;
    assert_eq!(response.result()["isError"], true, "{}", response.raw_body);
    assert!(
        text(response.result()).starts_with("invalid elicitation answer"),
        "{}",
        response.raw_body
    );
}

#[r2e_core::test]
async fn modes_come_from_the_requests_capabilities() {
    let router = keyed().await;
    for (capabilities, expected) in [
        (json!({ "elicitation": {} }), "form=true url=false"),
        (json!({ "elicitation": { "url": {} } }), "form=false url=true"),
        (json!({ "elicitation": { "form": {}, "url": {} } }), "form=true url=true"),
        (json!({}), "form=false url=false"),
    ] {
        let response = send(
            &router,
            "tools/call",
            "caps",
            json!({ "name": "caps", "arguments": {} }),
            capabilities,
        )
        .await;
        assert_eq!(text(response.result()), expected);
    }

    // No capability: a plain tool error, no round.
    let response = send(
        &router,
        "tools/call",
        "swallow",
        json!({ "name": "swallow", "arguments": {} }),
        json!({}),
    )
    .await;
    assert_eq!(text(response.result()), "err=Unsupported");
}

#[r2e_core::test]
async fn url_mode_is_keyed_by_the_elicitation_id() {
    let router = keyed().await;
    let params = json!({ "name": "consent", "arguments": {} });
    let capabilities = json!({ "elicitation": { "url": {} } });
    let first = send(&router, "tools/call", "consent", params.clone(), capabilities.clone()).await;
    let sealed = state(first.result(), &["consent-1"]);
    let request = &first.result()["inputRequests"]["consent-1"]["params"];
    assert_eq!(request["mode"], "url", "{request}");
    assert_eq!(request["url"], "https://auth.example.com/consent", "{request}");
    assert_eq!(request["elicitationId"], "consent-1", "{request}");

    let mut retry = params;
    retry["inputResponses"] = json!({ "consent-1": { "action": "accept" } });
    retry["requestState"] = json!(sealed);
    let second = send(&router, "tools/call", "consent", retry, capabilities).await;
    assert_eq!(text(second.result()), "Accept(())");
}

#[r2e_core::test]
async fn resources_and_prompts_take_rounds_too() {
    let router = keyed().await;
    let capabilities = json!({ "elicitation": {} });

    let params = json!({ "uri": "ask://secret" });
    let first = send(&router, "resources/read", "ask://secret", params.clone(), capabilities.clone()).await;
    let sealed = state(first.result(), &["reveal"]);
    let mut retry = params;
    retry["inputResponses"] = json!({ "reveal": accept(true) });
    retry["requestState"] = json!(sealed);
    let second = send(&router, "resources/read", "ask://secret", retry, capabilities.clone()).await;
    assert_eq!(second.result()["contents"][0]["text"], "s3cret", "{}", second.raw_body);

    let params = json!({ "name": "ask_prompt" });
    let first = send(&router, "prompts/get", "ask_prompt", params.clone(), capabilities.clone()).await;
    let sealed = state(first.result(), &["prompt"]);
    let mut retry = params;
    retry["inputResponses"] = json!({ "prompt": accept(false) });
    retry["requestState"] = json!(sealed);
    let second = send(&router, "prompts/get", "ask_prompt", retry, capabilities).await;
    assert_eq!(
        second.result()["messages"][0]["content"]["text"], "prompt:false",
        "{}",
        second.raw_body
    );
}

#[r2e_core::test]
async fn without_a_key_there_is_no_way_back() {
    let router = app(McpServer::new()).await;
    let response = tool(&router, "swallow", json!({})).await;
    assert_eq!(text(response.result()), "err=NoChannel");
    let response = tool(&router, "caps", json!({})).await;
    assert_eq!(text(response.result()), "form=false url=false");
}

#[r2e_core::test]
async fn legacy_sessions_keep_the_live_channel() {
    // A key changes nothing for a session-based client: no `input_required`.
    let router = keyed().await;
    let session = support::initialize(&router, "/mcp").await;
    let result = support::tools_call(&router, "/mcp", &session, "swallow", json!({})).await;
    // The handshake advertised no elicitation capability.
    assert_eq!(text(&result["result"]), "err=Unsupported");
}

#[r2e_core::test]
#[should_panic(expected = "mcp.request-state-key")]
async fn a_short_key_fails_the_boot() {
    app(McpServer::new().with_request_state_key("too-short")).await;
}
