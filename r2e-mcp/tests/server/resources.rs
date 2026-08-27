//! `#[resource]` members on the wire: capability advertisement,
//! `resources/list` metadata, `resources/read` dispatch (declared MIME type,
//! interceptors), and the resource error plane (JSON-RPC only — no
//! `is_error` results).

use r2e_core::http::{Router, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{AppBuilderMcpExt, McpServer};
use serde_json::Value;

use crate::fixtures::fixture_app;
use crate::support;

async fn initialize_result(router: &Router) -> Value {
    let response = support::post(router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    response.result().clone()
}

#[r2e_core::test]
async fn capabilities_advertise_resources_and_prompts_when_present() {
    let (router, _log) = fixture_app().await;
    let capabilities = &initialize_result(&router).await["capabilities"];
    assert!(capabilities.get("tools").is_some(), "{capabilities}");
    assert!(capabilities.get("resources").is_some(), "{capabilities}");
    assert!(capabilities.get("prompts").is_some(), "{capabilities}");
}

// A tools-only service, to prove absent families are NOT advertised.
#[controller]
struct ToolsOnly;

#[mcp_routes]
impl ToolsOnly {
    /// Reply with `pong`.
    #[tool]
    async fn ping(&self) -> String {
        "pong".to_string()
    }
}

#[r2e_core::test]
async fn absent_families_are_not_advertised() {
    let router = AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<ToolsOnly>()
        .build();
    let capabilities = &initialize_result(&router).await["capabilities"];
    assert!(capabilities.get("tools").is_some(), "{capabilities}");
    assert!(capabilities.get("resources").is_none(), "{capabilities}");
    assert!(capabilities.get("prompts").is_none(), "{capabilities}");
}

#[r2e_core::test]
async fn resources_are_listed_with_their_metadata() {
    let (router, _log) = fixture_app().await;
    let session = support::initialize(&router, "/mcp").await;
    let list = support::resources_list(&router, "/mcp", &session).await;

    // Name defaults to the method name; description is the doc comment.
    let log = support::resource(&list, "r2e://fixture/log");
    assert_eq!(log["name"], "call_log");
    assert_eq!(
        log["description"],
        "The interceptor call log, one entry per line."
    );
    assert_eq!(log["mimeType"], "text/plain");

    // Explicit name/title override; no declared MIME type.
    let failing = support::resource(&list, "r2e://fixture/fail");
    assert_eq!(failing["name"], "failing");
    assert_eq!(failing["title"], "Failing resource");
    assert!(failing.get("mimeType").is_none(), "{failing}");
}

#[r2e_core::test]
async fn read_returns_text_contents_with_the_declared_mime_type() {
    let (router, log) = fixture_app().await;
    let session = support::initialize(&router, "/mcp").await;
    let message = support::resources_read(&router, "/mcp", &session, "r2e://fixture/log").await;

    let contents = &message["result"]["contents"][0];
    assert_eq!(contents["uri"], "r2e://fixture/log");
    assert_eq!(contents["mimeType"], "text/plain");
    // The #[intercept] on the resource ran (and is the only log entry) —
    // decorators are shared with tools.
    assert_eq!(contents["text"], "res:call_log");
    assert_eq!(log.entries(), ["res:call_log"]);
}

#[r2e_core::test]
async fn unknown_resource_uri_is_resource_not_found() {
    let (router, _log) = fixture_app().await;
    let session = support::initialize(&router, "/mcp").await;
    let message = support::resources_read(&router, "/mcp", &session, "r2e://fixture/missing").await;
    assert_eq!(message["error"]["code"], -32002, "{message}");
    assert_eq!(
        message["error"]["message"],
        "unknown resource: r2e://fixture/missing"
    );
}

#[r2e_core::test]
async fn domain_error_degrades_to_a_json_rpc_internal_error() {
    let (router, _log) = fixture_app().await;
    let session = support::initialize(&router, "/mcp").await;
    let message = support::resources_read(&router, "/mcp", &session, "r2e://fixture/fail").await;
    // Resources have no in-result error plane: `McpError::Tool` becomes a
    // plain JSON-RPC internal error, message preserved.
    assert!(message.get("result").is_none(), "{message}");
    assert_eq!(message["error"]["code"], -32603, "{message}");
    assert_eq!(message["error"]["message"], "resource exploded");
}
