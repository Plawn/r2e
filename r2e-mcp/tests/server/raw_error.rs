//! `McpError::Rpc` (#1468): a handler returns a finished JSON-RPC error and
//! the client receives its code, message and `data` as written — from a
//! tool, a resource, a prompt and an extension method, catalog and dynamic
//! members alike. The named variants keep their own mapping.

use r2e_core::http::{Routes, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{
    AppBuilderMcpExt, DynamicPrompt, DynamicResource, DynamicTool, ErrorData, McpError,
    McpRegistry, McpServer, PromptCall, ResourceCall, ToolCall,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::support::{self, initialize};

const CODE: i32 = -32050;

fn upstream() -> McpError {
    McpError::rpc(CODE, "backend refused", Some(json!({ "backend": "b1", "retry": false })))
}

#[derive(Deserialize, JsonSchema, ObjectParams)]
pub struct NoArgs {}

#[controller]
pub struct Relay;

#[routes]
impl Relay {
    /// A tool that fails with its own code.
    #[tool]
    async fn relay_tool(&self) -> Result<&'static str, McpError> {
        Err(upstream())
    }

    /// `?` on an `ErrorData`.
    #[tool]
    async fn relay_question_mark(&self) -> Result<&'static str, McpError> {
        Err(ErrorData::new(rmcp::model::ErrorCode(CODE), "from ErrorData", None))?;
        Ok("unreachable")
    }

    /// A named variant next to it: unchanged.
    #[tool]
    async fn relay_named(&self) -> Result<&'static str, McpError> {
        Err(McpError::InvalidParams("bad".into()))
    }

    #[resource(uri = "relay://doc")]
    async fn relay_doc(&self) -> Result<&'static str, McpError> {
        Err(upstream())
    }

    #[prompt]
    async fn relay_prompt(&self) -> Result<&'static str, McpError> {
        Err(upstream())
    }

    #[method(name = "relay/do")]
    async fn relay_method(&self) -> Result<Value, McpError> {
        Err(upstream())
    }
}

async fn app() -> (Routes, McpRegistry) {
    let app = AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<Relay>();
    let registry = app.bean_context().try_get::<McpRegistry>().unwrap();
    registry
        .edit(|edit| {
            edit.add_tool(
                DynamicTool::new("dyn_tool")
                    .handler(|_p: NoArgs, _call: ToolCall| async { Err::<&str, _>(upstream()) }),
            )?;
            edit.add_resource(
                DynamicResource::new("dyn://doc", "doc")
                    .handler(|_call: ResourceCall| async { Err::<&str, _>(upstream()) }),
            )?;
            edit.add_prompt(
                DynamicPrompt::new("dyn_prompt")
                    .handler(|_p: NoArgs, _call: PromptCall| async { Err::<&str, _>(upstream()) }),
            )
        })
        .unwrap();
    (app.build(), registry)
}

async fn rpc(router: &Routes, session: &str, method: &str, params: Value) -> Value {
    let response = support::post_with_headers(
        router,
        "/mcp",
        Some(session),
        &[],
        &json!({ "jsonrpc": "2.0", "id": 3, "method": method, "params": params }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    response.message().clone()
}

fn assert_relayed(message: &Value) {
    let error = &message["error"];
    assert_eq!(error["code"], CODE, "{message}");
    assert_eq!(error["message"], "backend refused", "{message}");
    assert_eq!(error["data"], json!({ "backend": "b1", "retry": false }), "{message}");
}

#[r2e_core::test]
async fn a_raw_error_reaches_the_client_as_written() {
    let (router, _registry) = app().await;
    let sid = initialize(&router, "/mcp").await;

    for tool in ["relay_tool", "dyn_tool"] {
        let got = rpc(&router, &sid, "tools/call", json!({ "name": tool, "arguments": {} })).await;
        // A protocol error, not an `isError` result.
        assert!(got.get("result").is_none(), "{got}");
        assert_relayed(&got);
    }
    for uri in ["relay://doc", "dyn://doc"] {
        assert_relayed(&rpc(&router, &sid, "resources/read", json!({ "uri": uri })).await);
    }
    for prompt in ["relay_prompt", "dyn_prompt"] {
        let got = rpc(&router, &sid, "prompts/get", json!({ "name": prompt, "arguments": {} })).await;
        assert_relayed(&got);
    }
    assert_relayed(&rpc(&router, &sid, "relay/do", json!({})).await);

    let got = rpc(
        &router,
        &sid,
        "tools/call",
        json!({ "name": "relay_question_mark", "arguments": {} }),
    )
    .await;
    assert_eq!(got["error"]["code"], CODE, "{got}");
    assert_eq!(got["error"]["message"], "from ErrorData", "{got}");
    assert!(got["error"].get("data").is_none(), "{got}");
}

#[r2e_core::test]
async fn named_variants_keep_their_mapping() {
    let (router, _registry) = app().await;
    let sid = initialize(&router, "/mcp").await;
    let got = rpc(&router, &sid, "tools/call", json!({ "name": "relay_named", "arguments": {} })).await;
    assert_eq!(got["error"]["code"], -32602, "{got}");

    // The mapping itself, variant by variant.
    let code = |err: McpError| err.into_error_data().code.0;
    assert_eq!(code(McpError::InvalidParams("m".into())), -32602);
    assert_eq!(code(McpError::NotFound("m".into())), -32002);
    assert_eq!(code(McpError::Unauthorized("m".into())), -32600);
    assert_eq!(code(McpError::Forbidden("m".into())), -32600);
    assert_eq!(code(McpError::Internal("m".into())), -32603);
    assert_eq!(code(McpError::tool("m")), -32603);
    assert!(McpError::tool("m").into_call_result().is_ok());

    let raw = upstream();
    assert_eq!(raw.to_string(), "backend refused (code -32050)");
    assert!(matches!(raw.clone().into_call_result(), Err(e) if e.code.0 == CODE));
    assert_eq!(raw.into_error_data().code.0, CODE);
}

#[cfg(feature = "loopback")]
#[r2e_core::test]
async fn a_raw_error_crosses_the_loopback() {
    use r2e_mcp::McpLoopback;
    use rmcp::model::CallToolRequestParams;
    use rmcp::ServiceError;

    let app = AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<Relay>();
    let loopback = app.bean_context().try_get::<McpLoopback>().unwrap();
    let _router = app.build();
    let client = loopback.session("relay").connect().await.unwrap();
    match client.call_tool(CallToolRequestParams::new("relay_tool")).await {
        Err(ServiceError::McpError(error)) => {
            assert_eq!(error.code.0, CODE);
            assert_eq!(error.data, Some(json!({ "backend": "b1", "retry": false })));
        }
        other => panic!("expected a JSON-RPC error, got {other:?}"),
    }
}
