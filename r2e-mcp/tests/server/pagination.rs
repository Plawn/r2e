//! Cursor pagination of the four `*/list` results (`mcp.page-size`): a full
//! walk, the last page without `nextCursor`, invalid/stale cursors (-32602),
//! and the unpaginated default.

use r2e_core::http::{Router, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{AppBuilderMcpExt, McpError, McpServer, McpSession, ResourceCall};
use serde_json::{json, Value};

use crate::support;

#[controller]
pub struct Many;

#[mcp_routes]
impl Many {
    /// 1.
    #[tool]
    async fn t1(&self) -> &'static str {
        "1"
    }
    /// 2.
    #[tool]
    async fn t2(&self) -> &'static str {
        "2"
    }
    /// 3.
    #[tool]
    async fn t3(&self) -> &'static str {
        "3"
    }
    /// 4.
    #[tool]
    async fn t4(&self) -> &'static str {
        "4"
    }
    /// A.
    #[resource(uri = "many://a")]
    async fn ra(&self) -> &'static str {
        "a"
    }
    /// B.
    #[resource(uri = "many://b")]
    async fn rb(&self) -> &'static str {
        "b"
    }
    /// C.
    #[resource(uri = "many://c")]
    async fn rc(&self) -> &'static str {
        "c"
    }

    /// X.
    #[resource(uri = "x://{id}")]
    async fn tx(&self, call: ResourceCall) -> String {
        call.uri
    }
    /// Y.
    #[resource(uri = "y://{id}")]
    async fn ty(&self, call: ResourceCall) -> String {
        call.uri
    }
    /// Z.
    #[resource(uri = "z://{id}")]
    async fn tz(&self, call: ResourceCall) -> String {
        call.uri
    }

    /// P1.
    #[prompt]
    async fn p1(&self) -> &'static str {
        "1"
    }
    /// P2.
    #[prompt]
    async fn p2(&self) -> &'static str {
        "2"
    }
    /// P3.
    #[prompt]
    async fn p3(&self) -> &'static str {
        "3"
    }
}

#[controller]
pub struct Toggle;

#[mcp_routes]
impl Toggle {
    /// Reveal the `extra` group (mutates the tool list mid-walk).
    #[tool]
    async fn enable_extra(&self, session: McpSession) -> Result<&'static str, McpError> {
        session.enable_group("extra")?;
        Ok("enabled")
    }
}

#[controller]
pub struct Extra;

#[mcp_routes(group = "extra", opt_in)]
impl Extra {
    /// Hidden until enabled.
    #[tool]
    async fn hidden(&self) -> &'static str {
        "hidden"
    }
}

async fn app(plugin: McpServer) -> Router {
    AppBuilder::new()
        .plugin(plugin)
        .build_state()
        .await
        .register_mcp_service::<Many>()
        .register_mcp_service::<Toggle>()
        .register_mcp_service::<Extra>()
        .build()
}

async fn list(router: &Router, session: Option<&str>, method: &str, cursor: Option<&str>) -> Value {
    let params = match cursor {
        Some(cursor) => json!({ "cursor": cursor }),
        None => json!({}),
    };
    let response = support::post(
        router,
        "/mcp",
        session,
        &json!({ "jsonrpc": "2.0", "id": 9, "method": method, "params": params }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    response.message().clone()
}

/// Follow `nextCursor` to the end: the keys of each page.
async fn walk(
    router: &Router,
    session: Option<&str>,
    method: &str,
    family: &str,
) -> Vec<Vec<String>> {
    let mut pages = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let message = list(router, session, method, cursor.as_deref()).await;
        let result = &message["result"];
        pages.push(support::names(result, family));
        match result.get("nextCursor").and_then(Value::as_str) {
            Some(next) => cursor = Some(next.to_string()),
            None => return pages,
        }
        assert!(pages.len() < 20, "pagination does not terminate");
    }
}

#[r2e_core::test]
async fn every_family_walks_in_pages() {
    let router = app(McpServer::new().with_page_size(2)).await;
    let session = support::initialize(&router, "/mcp").await;

    let tools = walk(&router, Some(&session), "tools/list", "tools").await;
    assert_eq!(
        tools,
        [vec!["t1", "t2"], vec!["t3", "t4"], vec!["enable_extra"]],
        "{tools:?}"
    );
    for (method, family, len) in [
        ("resources/list", "resources", 3),
        ("resources/templates/list", "resourceTemplates", 3),
        ("prompts/list", "prompts", 3),
    ] {
        let pages = walk(&router, Some(&session), method, family).await;
        assert_eq!(pages.len(), 2, "{method}: {pages:?}");
        assert_eq!(pages[0].len(), 2);
        let all: Vec<String> = pages.concat();
        assert_eq!(all.len(), len, "{method}: {all:?}");
        let mut unique = all.clone();
        unique.dedup();
        assert_eq!(unique, all, "{method}: duplicates");
    }
}

#[r2e_core::test]
async fn exact_multiple_ends_without_a_cursor() {
    let router = app(McpServer::new().with_page_size(5)).await;
    let session = support::initialize(&router, "/mcp").await;
    let message = list(&router, Some(&session), "tools/list", None).await;
    assert_eq!(support::tool_names(&message["result"]).len(), 5);
    assert!(message["result"].get("nextCursor").is_none(), "{message}");
}

#[r2e_core::test]
async fn unpaginated_by_default() {
    let router = app(McpServer::new()).await;
    let session = support::initialize(&router, "/mcp").await;
    let message = list(&router, Some(&session), "tools/list", None).await;
    assert_eq!(support::tool_names(&message["result"]).len(), 5);
    assert!(message["result"].get("nextCursor").is_none(), "{message}");

    // `0` also means one page.
    let router = app(McpServer::new().with_page_size(0)).await;
    let session = support::initialize(&router, "/mcp").await;
    let message = list(&router, Some(&session), "tools/list", None).await;
    assert!(message["result"].get("nextCursor").is_none(), "{message}");
}

#[r2e_core::test]
async fn garbage_cursors_are_invalid_params() {
    let router = app(McpServer::new().with_page_size(2)).await;
    let session = support::initialize(&router, "/mcp").await;
    let first = list(&router, Some(&session), "tools/list", None).await;
    let good = first["result"]["nextCursor"].as_str().unwrap().to_string();
    let (_, rest) = good.split_once('.').unwrap();
    for cursor in [
        "garbage".to_string(),
        format!("v2.{rest}"),
        good.replacen(".2.", ".0.", 1),
        good.replacen(".2.", ".99.", 1),
        format!("{good}.extra"),
    ] {
        let message = list(&router, Some(&session), "tools/list", Some(&cursor)).await;
        assert_eq!(message["error"]["code"], -32602, "{cursor}: {message}");
    }

    // A cursor sent to an unpaginated server was not issued by it.
    let plain = app(McpServer::new()).await;
    let session = support::initialize(&plain, "/mcp").await;
    let message = list(&plain, Some(&session), "tools/list", Some(&good)).await;
    assert_eq!(message["error"]["code"], -32602, "{message}");
}

#[r2e_core::test]
async fn a_list_change_mid_walk_invalidates_the_cursor() {
    let router = app(McpServer::new().with_page_size(2)).await;
    let session = support::initialize(&router, "/mcp").await;
    let first = list(&router, Some(&session), "tools/list", None).await;
    let tools_cursor = first["result"]["nextCursor"].as_str().unwrap().to_string();
    let prompts = list(&router, Some(&session), "prompts/list", None).await;
    let prompts_cursor = prompts["result"]["nextCursor"]
        .as_str()
        .unwrap()
        .to_string();

    support::tools_call(&router, "/mcp", &session, "enable_extra", json!({})).await;

    let stale = list(&router, Some(&session), "tools/list", Some(&tools_cursor)).await;
    assert_eq!(stale["error"]["code"], -32602, "{stale}");
    // Another family's cursor survives: its list did not change.
    let still = list(
        &router,
        Some(&session),
        "prompts/list",
        Some(&prompts_cursor),
    )
    .await;
    assert_eq!(
        support::names(&still["result"], "prompts").len(),
        1,
        "{still}"
    );
}

#[r2e_core::test]
async fn stateless_servers_paginate_too() {
    // No session: the cursor carries everything a follow-up request needs.
    let router = AppBuilder::new()
        .plugin(McpServer::new().with_page_size(3).stateless(true))
        .build_state()
        .await
        .register_mcp_service::<Many>()
        .build();
    let pages = walk(&router, None, "tools/list", "tools").await;
    assert_eq!(pages, [vec!["t1", "t2", "t3"], vec!["t4"]], "{pages:?}");
}
