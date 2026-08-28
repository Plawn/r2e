//! `McpServer` plugin behavior: enable/disable gate, endpoint path
//! precedence and validation, transport modes.

use r2e_core::http::StatusCode;
use r2e_core::{AppBuilder, R2eConfig};
use r2e_mcp::{AppBuilderMcpExt, McpServer};
use serde_json::json;

use crate::fixtures::{fixture_app_with, Calc, CallLog, FixtureTools};
use crate::support;

fn config(yaml: &str) -> R2eConfig {
    R2eConfig::from_yaml_str(yaml).unwrap()
}

#[r2e_core::test]
async fn disabled_plugin_mounts_nothing_but_registration_still_works() {
    // `mcp.enabled = false` drops the endpoint — but the registry is
    // deposited ungated from setup(), so `register_mcp_service` must not
    // panic.
    let router = AppBuilder::new()
        .override_config(config("mcp:\n  enabled: false\n"))
        .load_config::<()>()
        .plugin(McpServer::new())
        .provide(Calc)
        .provide(CallLog::default())
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();

    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
}

#[r2e_core::test]
async fn no_registered_service_means_no_endpoint() {
    let router = AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .build();

    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
}

#[r2e_core::test]
async fn path_from_config_moves_the_endpoint() {
    let log = CallLog::default();
    let router = AppBuilder::new()
        .override_config(config("mcp:\n  path: /tools\n"))
        .load_config::<()>()
        .plugin(McpServer::new())
        .provide(Calc)
        .provide(log)
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();

    let session = support::initialize(&router, "/tools").await;
    assert!(!session.is_empty());
    let miss = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(miss.status, StatusCode::NOT_FOUND);
}

#[r2e_core::test]
async fn builder_path_overrides_config_path() {
    let log = CallLog::default();
    let router = AppBuilder::new()
        .override_config(config("mcp:\n  path: /from-config\n"))
        .load_config::<()>()
        .plugin(McpServer::new().with_path("/from-builder"))
        .provide(Calc)
        .provide(log)
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();

    support::initialize(&router, "/from-builder").await;
    let miss = support::post(&router, "/from-config", None, &support::initialize_body()).await;
    assert_eq!(miss.status, StatusCode::NOT_FOUND);
}

#[r2e_core::test]
#[should_panic(expected = "mcp.path must start with '/'")]
async fn relative_path_is_a_boot_error() {
    let _ = fixture_app_with(McpServer::new().with_path("no-slash")).await;
}

#[r2e_core::test]
#[should_panic(expected = "without a trailing slash")]
async fn trailing_slash_path_is_a_boot_error() {
    let _ = fixture_app_with(McpServer::new().with_path("/mcp/")).await;
}

#[r2e_core::test]
#[should_panic(expected = "no `{param}` captures or wildcards")]
async fn capture_path_is_a_boot_error() {
    let _ = fixture_app_with(McpServer::new().with_path("/mcp/{id}")).await;
}

#[r2e_core::test]
async fn server_identity_is_advertised_on_initialize() {
    let (router, _log) = fixture_app_with(
        McpServer::new()
            .with_name("fixture-server")
            .with_version("9.9.9")
            .with_instructions("call add"),
    )
    .await;

    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(response.status, StatusCode::OK);
    let result = response.result();
    assert_eq!(result["serverInfo"]["name"], "fixture-server");
    assert_eq!(result["serverInfo"]["version"], "9.9.9");
    assert_eq!(result["instructions"], "call add");
}

#[r2e_core::test]
async fn stateless_json_response_mode() {
    // `stateless` disables MCP sessions; `json-response` answers with plain
    // `application/json` instead of an SSE stream.
    let (router, _log) =
        fixture_app_with(McpServer::new().stateless(true).json_response(true)).await;

    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    assert!(
        response.content_type.starts_with("application/json"),
        "expected a plain JSON response, got {}",
        response.content_type
    );
    assert!(
        response.session_id.is_none(),
        "stateless mode must not mint sessions"
    );

    // No session header needed on subsequent requests.
    let call = support::post(
        &router,
        "/mcp",
        None,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "add", "arguments": { "a": 2.0, "b": 3.0 } }
        }),
    )
    .await;
    assert_eq!(call.status, StatusCode::OK, "{}", call.raw_body);
    assert_eq!(call.result()["structuredContent"]["value"], 5.0);
}

