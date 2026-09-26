//! `completion/complete` against scope-gated members: a caller lacking the
//! referenced member's scopes gets the unknown-reference error (no
//! existence leak), a caller holding them gets the suggestions.

use r2e_core::http::Router;
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{AppBuilderMcpExt, Completion, McpServer, Params, ResourceCall};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::fixtures::{initialize_auth, offline_auth, pinned, rpc_auth, test_jwt};

#[derive(Deserialize, JsonSchema, ObjectParams)]
pub struct DraftIn {
    pub topic: String,
}

#[controller]
pub struct GatedCompletions;

#[mcp_routes]
impl GatedCompletions {
    /// Scope-gated prompt with a completed argument.
    #[prompt(scopes = "mcp:write", complete(topic = "topics"))]
    async fn draft(&self, Params(p): Params<DraftIn>) -> String {
        p.topic
    }

    /// Scope-gated resource template with a completed variable.
    #[resource(uri = "r2e://gated/{id}", scopes = "mcp:write", complete(id = "ids"))]
    async fn gated(&self, call: ResourceCall) -> String {
        call.variables["id"].clone()
    }

    #[completion]
    async fn topics(&self, _c: Completion) -> Vec<String> {
        vec!["release".into()]
    }

    #[completion]
    async fn ids(&self, _c: Completion) -> Vec<String> {
        vec!["42".into()]
    }
}

async fn app() -> Router {
    AppBuilder::new()
        .plugin(
            McpServer::new()
                .with_auth(offline_auth())
                .with_token_validator(pinned(&test_jwt())),
        )
        .build_state()
        .await
        .register_mcp_service::<GatedCompletions>()
        .build()
}

async fn complete(router: &Router, token: &str, reference: Value, argument: &str) -> Value {
    let session = initialize_auth(router, "/mcp", token).await;
    rpc_auth(
        router,
        "/mcp",
        &session,
        token,
        "completion/complete",
        json!({ "ref": reference, "argument": { "name": argument, "value": "" } }),
    )
    .await
}

#[r2e_core::test]
async fn missing_scope_looks_like_an_unknown_reference() {
    let router = app().await;
    let reader = test_jwt()
        .token_builder("alice")
        .scopes(&["mcp:read"])
        .build();
    let unknown = complete(
        &router,
        &reader,
        json!({ "type": "ref/prompt", "name": "nope" }),
        "topic",
    )
    .await;
    assert_eq!(
        unknown["error"]["message"], "unknown completion reference",
        "{unknown}"
    );
    for (reference, argument) in [
        (json!({ "type": "ref/prompt", "name": "draft" }), "topic"),
        (
            json!({ "type": "ref/resource", "uri": "r2e://gated/{id}" }),
            "id",
        ),
    ] {
        let hidden = complete(&router, &reader, reference, argument).await;
        assert_eq!(hidden["error"], unknown["error"], "{hidden}");
    }
}

#[r2e_core::test]
async fn holding_the_scope_gets_suggestions() {
    let router = app().await;
    let writer = test_jwt()
        .token_builder("bob")
        .scopes(&["mcp:write"])
        .build();
    let prompt = complete(
        &router,
        &writer,
        json!({ "type": "ref/prompt", "name": "draft" }),
        "topic",
    )
    .await;
    assert_eq!(
        prompt["result"]["completion"]["values"],
        json!(["release"]),
        "{prompt}"
    );
    let resource = complete(
        &router,
        &writer,
        json!({ "type": "ref/resource", "uri": "r2e://gated/{id}" }),
        "id",
    )
    .await;
    assert_eq!(
        resource["result"]["completion"]["values"],
        json!(["42"]),
        "{resource}"
    );
}
