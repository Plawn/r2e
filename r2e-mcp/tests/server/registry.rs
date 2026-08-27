//! `McpServiceRegistry` semantics and the duplicate-tool-name boot panic.

use std::borrow::Cow;
use std::sync::Arc;

use r2e_mcp::{IntoToolResult, McpServiceRegistry, ToolRoute};

use crate::fixtures::{fixture_app, FixtureTools};
use r2e_core::AppBuilder;
use r2e_mcp::{AppBuilderMcpExt, McpServer};

fn stub_tool(name: &'static str) -> ToolRoute {
    ToolRoute {
        name: Cow::Borrowed(name),
        title: None,
        description: None,
        input_schema: Arc::new(serde_json::Map::new()),
        output_schema: None,
        annotations: Default::default(),
        invoke: Arc::new(|_call| Box::pin(async { ().into_tool_result() })),
    }
}

#[test]
fn take_on_empty_registry_is_none() {
    let registry = McpServiceRegistry::new();
    assert!(registry.take().is_none());
}

#[test]
fn add_service_then_take_drains_once() {
    let registry = McpServiceRegistry::new();
    registry.add_service("svc-a", vec![stub_tool("a")]);
    registry.add_service("svc-b", vec![stub_tool("b"), stub_tool("c")]);
    assert_eq!(registry.service_names(), vec!["svc-a", "svc-b"]);

    let drained = registry.take().expect("filled registry drains Some");
    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].name, "svc-a");
    assert_eq!(drained[1].tools.len(), 2);

    // Drained means drained: the second take is None again.
    assert!(registry.take().is_none());
    assert!(registry.service_names().is_empty());
}

#[test]
fn clones_share_the_same_registry() {
    // The plugin stores a clone in plugin data; `register_mcp_service` fills
    // through that clone and `wrap_router` drains through the original.
    let registry = McpServiceRegistry::new();
    let clone = registry.clone();
    clone.add_service("svc", vec![stub_tool("t")]);
    assert_eq!(registry.service_names(), vec!["svc"]);
}

#[r2e_core::test]
#[should_panic(expected = "duplicate MCP tool name")]
async fn duplicate_tool_name_across_services_panics_at_boot() {
    // Registering the same service twice registers every tool name twice —
    // the MCP equivalent of an HTTP route conflict, surfaced when the router
    // is assembled.
    let _ = AppBuilder::new()
        .plugin(McpServer::new())
        .provide(crate::fixtures::Calc)
        .provide(crate::fixtures::CallLog::default())
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .register_mcp_service::<FixtureTools>()
        .build();
}

#[r2e_core::test]
async fn service_tools_are_all_mounted() {
    let (router, _log) = fixture_app().await;
    let session = crate::support::initialize(&router, "/mcp").await;
    let list = crate::support::tools_list(&router, "/mcp", &session).await;
    let mut names: Vec<&str> = list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(names, vec!["add", "div", "echo_id", "locked", "rich"]);
}
