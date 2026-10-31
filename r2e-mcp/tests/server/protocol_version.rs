//! `mcp.max-protocol-version` / `McpServer::max_protocol_version` (#1183):
//! one cap behind `server/discover`, the `initialize` negotiation and the
//! per-request `_meta` version gate.

use r2e_core::http::StatusCode;
use r2e_core::{AppBuilder, R2eConfig};
use r2e_mcp::{AppBuilderMcpExt, McpServer, ProtocolVersion};
use serde_json::{json, Value};

use crate::fixtures::{fixture_app_with, Calc, CallLog, FixtureTools};
use crate::support;

const ALL_KNOWN: [&str; 5] = [
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    "2025-11-25",
    "2026-07-28",
];

/// The `_meta` a 2026-07-28-style (SEP-2567) request carries in place of the
/// `initialize` handshake.
fn meta(version: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

/// `server/discover`, negotiating `version` inline.
async fn discover(router: &r2e_core::http::Routes, version: &str) -> support::McpResponse {
    support::post_with_headers(
        router,
        "/mcp",
        None,
        &[
            ("mcp-protocol-version", version),
            ("mcp-method", "server/discover"),
        ],
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "server/discover",
            "params": { "_meta": meta(version) }
        }),
    )
    .await
}

fn supported_versions(result: &Value) -> Vec<&str> {
    result["supportedVersions"]
        .as_array()
        .expect("supportedVersions array")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect()
}

async fn initialize_as(router: &r2e_core::http::Routes, version: &str) -> support::McpResponse {
    let mut body = support::initialize_body();
    body["params"]["protocolVersion"] = json!(version);
    support::post(router, "/mcp", None, &body).await
}

fn config(yaml: &str) -> R2eConfig {
    R2eConfig::from_yaml_str(yaml).unwrap()
}

#[r2e_core::test]
async fn uncapped_server_advertises_every_known_version() {
    let (router, _log) = fixture_app().await;

    let response = discover(&router, "2026-07-28").await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    assert_eq!(supported_versions(response.result()), ALL_KNOWN);
}

#[r2e_core::test]
async fn cap_hides_newer_versions_from_discover() {
    let (router, _log) =
        fixture_app_with(McpServer::new().max_protocol_version(ProtocolVersion::V_2025_11_25))
            .await;

    let response = discover(&router, "2025-11-25").await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    assert_eq!(supported_versions(response.result()), &ALL_KNOWN[..4]);
}

#[r2e_core::test]
async fn request_above_the_cap_is_refused_with_the_supported_list() {
    let (router, _log) =
        fixture_app_with(McpServer::new().max_protocol_version(ProtocolVersion::V_2025_11_25))
            .await;

    let response = support::post_with_headers(
        &router,
        "/mcp",
        None,
        &[
            ("mcp-protocol-version", "2026-07-28"),
            ("mcp-method", "tools/list"),
        ],
        &json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/list",
            "params": { "_meta": meta("2026-07-28") }
        }),
    )
    .await;
    let error = &response.message()["error"];
    assert_eq!(error["code"], -32022, "{}", response.raw_body);
    assert_eq!(error["data"]["requested"], "2026-07-28");
    assert_eq!(error["data"]["supported"], json!(&ALL_KNOWN[..4]));
}

#[r2e_core::test]
async fn initialize_negotiates_within_the_cap() {
    let (router, _log) =
        fixture_app_with(McpServer::new().max_protocol_version(ProtocolVersion::V_2025_06_18))
            .await;

    // A supported revision is echoed back.
    let response = initialize_as(&router, "2025-03-26").await;
    assert_eq!(response.result()["protocolVersion"], "2025-03-26");

    // One above the cap falls back to the newest supported handshake revision.
    let response = initialize_as(&router, "2025-11-25").await;
    assert_eq!(response.result()["protocolVersion"], "2025-06-18");
    assert!(
        response.session_id.is_some(),
        "capped servers still mint Mcp-Session-Id sessions"
    );
}

#[r2e_core::test]
async fn uncapped_initialize_asking_for_the_newest_revision_falls_back() {
    // 2026-07-28 has no `initialize` handshake, so even an uncapped server
    // answers with the newest revision that does.
    let (router, _log) = fixture_app().await;

    let response = initialize_as(&router, "2026-07-28").await;
    assert_eq!(response.result()["protocolVersion"], "2025-11-25");
}

#[r2e_core::test]
async fn cap_from_config_and_builder_precedence() {
    let router = AppBuilder::new()
        .override_config(config("mcp:\n  max-protocol-version: \"2025-11-25\"\n"))
        .load_config::<()>()
        .plugin(McpServer::new())
        .provide(Calc)
        .provide(CallLog::default())
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();
    let response = discover(&router, "2025-11-25").await;
    assert_eq!(supported_versions(response.result()), &ALL_KNOWN[..4]);

    let router = AppBuilder::new()
        .override_config(config("mcp:\n  max-protocol-version: \"2025-11-25\"\n"))
        .load_config::<()>()
        .plugin(McpServer::new().max_protocol_version(ProtocolVersion::V_2025_06_18))
        .provide(Calc)
        .provide(CallLog::default())
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();
    let response = discover(&router, "2025-06-18").await;
    assert_eq!(supported_versions(response.result()), &ALL_KNOWN[..3]);
}

#[r2e_core::test]
#[should_panic(
    expected = "mcp.max-protocol-version must be a known MCP protocol revision (2024-11-05, 2025-03-26, 2025-06-18, 2025-11-25, 2026-07-28): got `2027-01-01`"
)]
async fn unknown_cap_is_a_boot_error() {
    let _ = AppBuilder::new()
        .override_config(config("mcp:\n  max-protocol-version: \"2027-01-01\"\n"))
        .load_config::<()>()
        .plugin(McpServer::new())
        .build_state()
        .await;
}

#[r2e_core::test]
#[should_panic(expected = "got `latest`")]
async fn alias_cap_is_a_boot_error() {
    let _ = AppBuilder::new()
        .override_config(config("mcp:\n  max-protocol-version: latest\n"))
        .load_config::<()>()
        .plugin(McpServer::new())
        .build_state()
        .await;
}

async fn fixture_app() -> (r2e_core::http::Routes, CallLog) {
    fixture_app_with(McpServer::new()).await
}
