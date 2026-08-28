//! `McpServiceRegistry` semantics and the duplicate-tool-name boot panic.

use std::borrow::Cow;
use std::sync::Arc;

use r2e_core::prelude::*;
use r2e_mcp::{IntoToolResult, McpRoutes, McpServiceRegistry, Params, ToolRequirements, ToolRoute};
use serde_json::json;

use crate::fixtures::{fixture_app, BinaryOperands, Calc, CalcResult, FixtureTools};
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
        requirements: ToolRequirements::NONE,
        invoke: Arc::new(|_call| Box::pin(async { ().into_tool_result() })),
    }
}

#[controller]
pub struct ScopedWithoutAuth;

#[mcp_routes]
impl ScopedWithoutAuth {
    #[tool(scopes = "mcp:read")]
    async fn restricted(&self) -> String {
        "secret".to_string()
    }
}

#[derive(Clone)]
struct BeanTools {
    calc: Calc,
}

#[bean]
impl BeanTools {
    fn new(calc: Calc) -> Self {
        Self { calc }
    }

    #[tool(name = "bean_add", read_only)]
    async fn add(&self, Params(p): Params<BinaryOperands>) -> Json<CalcResult> {
        Json(CalcResult {
            value: self.calc.add(p.a, p.b),
        })
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
    registry.add_service("svc-a", McpRoutes::from_tools(vec![stub_tool("a")]));
    registry.add_service(
        "svc-b",
        McpRoutes::from_tools(vec![stub_tool("b"), stub_tool("c")]),
    );
    assert_eq!(registry.service_names(), vec!["svc-a", "svc-b"]);

    let drained = registry.take().expect("filled registry drains Some");
    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].name, "svc-a");
    assert_eq!(drained[1].routes.tools.len(), 2);

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
    clone.add_service("svc", McpRoutes::from_tools(vec![stub_tool("t")]));
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
#[should_panic(expected = "declares OAuth scopes but `mcp.auth` is disabled")]
async fn scoped_member_without_auth_panics_at_boot() {
    let _ = AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<ScopedWithoutAuth>()
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
    assert_eq!(
        names,
        vec!["add", "add_with_id", "div", "echo_id", "locked", "rich"]
    );
}

#[r2e_core::test]
async fn bean_tools_are_collected_without_explicit_service_registration() {
    let router = AppBuilder::new()
        .plugin(McpServer::new())
        .provide(Calc)
        .register::<BeanTools>()
        .build_state()
        .await
        .build();

    let session = crate::support::initialize(&router, "/mcp").await;
    let list = crate::support::tools_list(&router, "/mcp", &session).await;
    assert_eq!(crate::support::tool(&list, "bean_add")["name"], "bean_add");

    let call = crate::support::tools_call(
        &router,
        "/mcp",
        &session,
        "bean_add",
        json!({ "a": 2.0, "b": 5.0 }),
    )
    .await;
    assert_eq!(call["result"]["structuredContent"]["value"], 7.0);
}
