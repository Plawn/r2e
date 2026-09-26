//! `completion/complete`: `#[completion]` providers wired through
//! `complete(arg = "method")` on prompts and resource templates, dynamic
//! members' `with_completion`, the 100-value cap, the `completions`
//! capability, and the unknown/hidden-reference error.

use r2e_core::http::{Router, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{
    AppBuilderMcpExt, Completion, CompletionRef, Completions, DynamicPrompt, DynamicResource,
    McpError, McpServer, McpSession, Params, PromptCall, ResourceCall,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::fixtures::fixture_app_with;
use crate::support;

#[derive(Deserialize, JsonSchema, ObjectParams)]
pub struct ReviewIn {
    pub lang: String,
    pub style: Option<String>,
}

#[controller]
pub struct Completer;

#[mcp_routes]
impl Completer {
    /// Review code in a language (`lang` is completed).
    #[prompt(complete(lang = "languages"))]
    async fn review(&self, Params(p): Params<ReviewIn>) -> String {
        format!(
            "Review this {} code. {}",
            p.lang,
            p.style.unwrap_or_default()
        )
    }

    /// Same arguments, no provider.
    #[prompt]
    async fn plain(&self, Params(p): Params<ReviewIn>) -> String {
        p.lang
    }

    /// Both template variables are completed; `name` depends on `dir`.
    #[resource(
        uri = "files://{dir}/{name}",
        complete(dir = "dirs", name = "file_names")
    )]
    async fn file(&self, call: ResourceCall) -> String {
        format!("{}/{}", call.variables["dir"], call.variables["name"])
    }

    /// Far more matches than the wire allows.
    #[resource(uri = "big://{item}", complete(item = "many"))]
    async fn big(&self, call: ResourceCall) -> String {
        call.variables["item"].clone()
    }

    /// The provider fails.
    #[resource(uri = "fail://{x}", complete(x = "broken"))]
    async fn fail(&self, call: ResourceCall) -> String {
        call.variables["x"].clone()
    }

    #[completion]
    async fn languages(&self, c: Completion) -> Vec<String> {
        assert_eq!(c.reference, CompletionRef::Prompt("review".into()));
        ["python", "ruby", "rust"]
            .into_iter()
            .filter(|l| l.starts_with(&c.value))
            .map(String::from)
            .collect()
    }

    #[completion]
    async fn dirs(&self, _c: Completion) -> Vec<String> {
        vec!["docs".into(), "src".into()]
    }

    #[completion]
    async fn file_names(&self, c: Completion) -> Completions {
        match c.context.get("dir").map(String::as_str) {
            Some("docs") => Completions::new(["guide.md", "intro.md"]),
            Some("src") => Completions::new(["lib.rs"]),
            _ => Completions::empty(),
        }
    }

    #[completion]
    async fn many(&self, _c: Completion) -> Vec<String> {
        (0..150).map(|i| format!("item-{i}")).collect()
    }

    #[completion]
    async fn broken(&self, _c: Completion) -> Result<Vec<String>, McpError> {
        Err(McpError::InvalidParams("no suggestions for you".into()))
    }

    /// Add a session-private prompt and resource with completion.
    #[tool]
    async fn add_dynamic(&self, session: McpSession) -> Result<&'static str, McpError> {
        session.add_prompt(
            DynamicPrompt::new("greet")
                .with_completion("lang", |c: Completion| async move {
                    vec![format!("{}-dyn", c.value)]
                })
                .handler(|p: ReviewIn, _call: PromptCall| async move { p.lang }),
        )?;
        session.add_resource(
            DynamicResource::new("notes://{topic}", "notes")
                .with_completion("topic", |_c: Completion| async {
                    Completions::new(["todo"])
                })
                .handler(|call: ResourceCall| async move { call.uri }),
        )?;
        Ok("added")
    }

    /// Reveal the opt-in `secret` group to this session.
    #[tool]
    async fn enable_secret(&self, session: McpSession) -> Result<&'static str, McpError> {
        session.enable_group("secret")?;
        Ok("enabled")
    }

    /// Try to add members whose providers name nothing completable.
    #[tool]
    async fn add_invalid(&self, session: McpSession) -> String {
        let prompt = session
            .add_prompt(
                DynamicPrompt::new("bad_prompt")
                    .with_completion("missing", |_c: Completion| async { Vec::<String>::new() })
                    .handler(|p: ReviewIn, _call: PromptCall| async move { p.lang }),
            )
            .unwrap_err();
        let resource = session
            .add_resource(
                DynamicResource::new("bad://fixed", "bad_resource")
                    .with_completion("x", |_c: Completion| async { Vec::<String>::new() })
                    .handler(|call: ResourceCall| async move { call.uri }),
            )
            .unwrap_err();
        format!("{prompt}|{resource}")
    }
}

#[controller]
pub struct SecretPrompts;

#[mcp_routes(group = "secret", opt_in)]
impl SecretPrompts {
    /// Only visible once the `secret` group is enabled.
    #[prompt(complete(lang = "secret_languages"))]
    async fn secret_review(&self, Params(p): Params<ReviewIn>) -> String {
        p.lang
    }

    #[completion]
    async fn secret_languages(&self, _c: Completion) -> Vec<String> {
        vec!["classified".into()]
    }
}

async fn app() -> Router {
    AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<Completer>()
        .build()
}

async fn app_with_secret() -> Router {
    AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<Completer>()
        .register_mcp_service::<SecretPrompts>()
        .build()
}

/// `completion/complete` → the full JSON-RPC message.
async fn complete(router: &Router, session: &str, reference: Value, argument: Value) -> Value {
    complete_with(
        router,
        session,
        json!({ "ref": reference, "argument": argument }),
    )
    .await
}

async fn complete_with(router: &Router, session: &str, params: Value) -> Value {
    let response = support::post(
        router,
        "/mcp",
        Some(session),
        &json!({ "jsonrpc": "2.0", "id": 9, "method": "completion/complete", "params": params }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    response.message().clone()
}

fn prompt_ref(name: &str) -> Value {
    json!({ "type": "ref/prompt", "name": name })
}

fn resource_ref(uri: &str) -> Value {
    json!({ "type": "ref/resource", "uri": uri })
}

fn arg(name: &str, value: &str) -> Value {
    json!({ "name": name, "value": value })
}

async fn capabilities(router: &Router) -> Value {
    let response = support::post(router, "/mcp", None, &support::initialize_body()).await;
    response.result()["capabilities"].clone()
}

#[r2e_core::test]
async fn prompt_argument_is_completed() {
    let router = app().await;
    let session = support::initialize(&router, "/mcp").await;
    let message = complete(&router, &session, prompt_ref("review"), arg("lang", "ru")).await;
    assert_eq!(
        message["result"]["completion"],
        json!({ "values": ["ruby", "rust"] }),
        "{message}"
    );
}

#[r2e_core::test]
async fn template_variable_completion_sees_the_context() {
    let router = app().await;
    let session = support::initialize(&router, "/mcp").await;
    let message = complete_with(
        &router,
        &session,
        json!({
            "ref": resource_ref("files://{dir}/{name}"),
            "argument": arg("name", ""),
            "context": { "arguments": { "dir": "docs" } },
        }),
    )
    .await;
    assert_eq!(
        message["result"]["completion"]["values"],
        json!(["guide.md", "intro.md"]),
        "{message}"
    );

    let message = complete(
        &router,
        &session,
        resource_ref("files://{dir}/{name}"),
        arg("dir", ""),
    )
    .await;
    assert_eq!(
        message["result"]["completion"]["values"],
        json!(["docs", "src"])
    );
}

#[r2e_core::test]
async fn template_is_matched_by_shape_when_spelled_differently() {
    let router = app().await;
    let session = support::initialize(&router, "/mcp").await;
    // Same shape, different variable names: still the `file` template, whose
    // `dir` provider answers for the argument named `dir`.
    let message = complete(
        &router,
        &session,
        resource_ref("files://{a}/{b}"),
        arg("dir", ""),
    )
    .await;
    assert_eq!(
        message["result"]["completion"]["values"],
        json!(["docs", "src"]),
        "{message}"
    );
}

#[r2e_core::test]
async fn values_are_capped_at_one_hundred() {
    let router = app().await;
    let session = support::initialize(&router, "/mcp").await;
    let message = complete(
        &router,
        &session,
        resource_ref("big://{item}"),
        arg("item", ""),
    )
    .await;
    let completion = &message["result"]["completion"];
    assert_eq!(
        completion["values"].as_array().unwrap().len(),
        100,
        "{message}"
    );
    assert_eq!(completion["values"][0], "item-0");
    assert_eq!(completion["total"], 150);
    assert_eq!(completion["hasMore"], true);
}

#[r2e_core::test]
async fn argument_without_provider_gets_no_suggestion() {
    let router = app().await;
    let session = support::initialize(&router, "/mcp").await;
    for (reference, argument) in [
        (prompt_ref("plain"), arg("lang", "r")),
        (prompt_ref("review"), arg("style", "t")),
    ] {
        let message = complete(&router, &session, reference, argument).await;
        assert_eq!(
            message["result"]["completion"]["values"],
            json!([]),
            "{message}"
        );
    }
}

#[r2e_core::test]
async fn unknown_reference_is_invalid_params() {
    let router = app().await;
    let session = support::initialize(&router, "/mcp").await;
    for reference in [prompt_ref("nope"), resource_ref("nope://{x}")] {
        let message = complete(&router, &session, reference, arg("x", "")).await;
        assert_eq!(message["error"]["code"], -32602, "{message}");
        assert_eq!(message["error"]["message"], "unknown completion reference");
    }
}

#[r2e_core::test]
async fn hidden_prompt_looks_unknown_until_its_group_is_enabled() {
    let router = app_with_secret().await;
    let session = support::initialize(&router, "/mcp").await;
    let hidden = complete(
        &router,
        &session,
        prompt_ref("secret_review"),
        arg("lang", ""),
    )
    .await;
    let unknown = complete(&router, &session, prompt_ref("nope"), arg("lang", "")).await;
    assert_eq!(hidden["error"], unknown["error"], "{hidden}");

    let _ = support::tools_call(&router, "/mcp", &session, "enable_secret", json!({})).await;
    let visible = complete(
        &router,
        &session,
        prompt_ref("secret_review"),
        arg("lang", ""),
    )
    .await;
    assert_eq!(
        visible["result"]["completion"]["values"],
        json!(["classified"]),
        "{visible}"
    );
}

#[r2e_core::test]
async fn provider_error_is_a_json_rpc_error() {
    let router = app().await;
    let session = support::initialize(&router, "/mcp").await;
    let message = complete(&router, &session, resource_ref("fail://{x}"), arg("x", "")).await;
    assert_eq!(message["error"]["code"], -32602, "{message}");
    assert!(
        message["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no suggestions for you"),
        "{message}"
    );
}

#[r2e_core::test]
async fn capability_follows_the_providers() {
    let with = capabilities(&app().await).await;
    assert!(with.get("completions").is_some(), "{with}");

    let (plain, _log) = fixture_app_with(McpServer::new().stateless(true)).await;
    let without = capabilities(&plain).await;
    assert!(without.get("completions").is_none(), "{without}");
}

#[r2e_core::test]
async fn dynamic_members_complete_through_with_completion() {
    let router = app().await;
    let session = support::initialize(&router, "/mcp").await;
    let before = complete(&router, &session, prompt_ref("greet"), arg("lang", "go")).await;
    assert_eq!(before["error"]["code"], -32602, "{before}");

    let added = support::tools_call(&router, "/mcp", &session, "add_dynamic", json!({})).await;
    assert_eq!(added["result"]["content"][0]["text"], "added", "{added}");

    let prompt = complete(&router, &session, prompt_ref("greet"), arg("lang", "go")).await;
    assert_eq!(
        prompt["result"]["completion"]["values"],
        json!(["go-dyn"]),
        "{prompt}"
    );
    let resource = complete(
        &router,
        &session,
        resource_ref("notes://{topic}"),
        arg("topic", ""),
    )
    .await;
    assert_eq!(
        resource["result"]["completion"]["values"],
        json!(["todo"]),
        "{resource}"
    );

    // Session-private: another session sees neither.
    let other = support::initialize(&router, "/mcp").await;
    let hidden = complete(&router, &other, prompt_ref("greet"), arg("lang", "go")).await;
    assert_eq!(hidden["error"]["code"], -32602, "{hidden}");
}

#[r2e_core::test]
async fn session_refuses_providers_naming_nothing_completable() {
    let router = app().await;
    let session = support::initialize(&router, "/mcp").await;
    let result = support::tools_call(&router, "/mcp", &session, "add_invalid", json!({})).await;
    let text = result["result"]["content"][0]["text"].as_str().unwrap();
    let (prompt, resource) = text.split_once('|').unwrap();
    assert!(prompt.contains("missing"), "{text}");
    assert!(resource.contains("bad://fixed"), "{text}");
}

#[r2e_core::test]
async fn hand_built_completion_has_no_session() {
    let call = Completion::new(CompletionRef::Prompt("review".into()), "lang", "r");
    assert!(call.session.is_none());
    assert!(call.context.is_empty());
}
