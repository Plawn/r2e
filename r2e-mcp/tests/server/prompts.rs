//! `#[prompt]` members on the wire: `prompts/list` with arguments derived
//! from the `Params<T>` schema, `prompts/get` expansion, and the prompt
//! error plane (JSON-RPC only).

use serde_json::{json, Value};

use crate::fixtures::fixture_app;
use crate::support;

fn argument<'a>(prompt: &'a Value, name: &str) -> &'a Value {
    prompt["arguments"]
        .as_array()
        .expect("prompt has no arguments array")
        .iter()
        .find(|a| a["name"] == name)
        .unwrap_or_else(|| panic!("argument `{name}` not declared in {prompt}"))
}

#[r2e_core::test]
async fn prompts_are_listed_with_schema_derived_arguments() {
    let (router, _log) = fixture_app().await;
    let session = support::initialize(&router, "/mcp").await;
    let list = support::prompts_list(&router, "/mcp", &session).await;

    let prompt = support::prompt(&list, "explain_div");
    // Description = verbatim doc text, like tools.
    assert_eq!(
        prompt["description"],
        "Explain a division.\n\nWalks the agent through dividing `a` by `b`."
    );
    // Arguments come from the Params<BinaryOperands> schema: names,
    // requiredness, and the field doc comments.
    let a = argument(prompt, "a");
    assert_eq!(a["description"], "Left operand.");
    assert_eq!(a["required"], true);
    let b = argument(prompt, "b");
    assert_eq!(b["description"], "Right operand.");
    assert_eq!(b["required"], true);
}

#[r2e_core::test]
async fn prompt_without_params_declares_no_arguments() {
    let (router, _log) = fixture_app().await;
    let session = support::initialize(&router, "/mcp").await;
    let list = support::prompts_list(&router, "/mcp", &session).await;
    let usage = support::prompt(&list, "usage");
    assert!(usage.get("arguments").is_none(), "{usage}");
    assert_eq!(usage["description"], "Static usage guidance.");
}

#[r2e_core::test]
async fn get_expands_the_prompt_with_its_description() {
    let (router, log) = fixture_app().await;
    let session = support::initialize(&router, "/mcp").await;
    let message = support::prompts_get(
        &router,
        "/mcp",
        &session,
        "explain_div",
        json!({ "a": 6.0, "b": 3.0 }),
    )
    .await;

    let result = &message["result"];
    assert_eq!(
        result["description"],
        "Explain a division.\n\nWalks the agent through dividing `a` by `b`."
    );
    let msg = &result["messages"][0];
    assert_eq!(msg["role"], "user");
    assert_eq!(msg["content"]["type"], "text");
    assert_eq!(msg["content"]["text"], "Divide 6 by 3 using the `div` tool.");
    // The #[intercept] on the prompt ran — decorators are shared with tools.
    assert_eq!(log.entries(), ["prompt:explain_division"]);
}

#[r2e_core::test]
async fn unknown_prompt_name_is_invalid_params() {
    let (router, _log) = fixture_app().await;
    let session = support::initialize(&router, "/mcp").await;
    let message = support::prompts_get(&router, "/mcp", &session, "nope", json!({})).await;
    assert_eq!(message["error"]["code"], -32602, "{message}");
    assert_eq!(message["error"]["message"], "unknown prompt: nope");
}

#[r2e_core::test]
async fn missing_required_argument_is_invalid_params() {
    let (router, _log) = fixture_app().await;
    let session = support::initialize(&router, "/mcp").await;
    let message = support::prompts_get(&router, "/mcp", &session, "explain_div", json!({})).await;
    assert_eq!(message["error"]["code"], -32602, "{message}");
    let text = message["error"]["message"].as_str().unwrap();
    assert!(text.contains("a"), "{text}");
}
