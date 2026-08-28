//! Generated tool metadata on the wire: input/output schemas, descriptions
//! from doc comments, annotations.

use serde_json::Value;

use crate::fixtures::fixture_app;
use crate::support::{initialize, tool, tools_list};

async fn listed() -> Value {
    let (router, _log) = fixture_app().await;
    let session = initialize(&router, "/mcp").await;
    tools_list(&router, "/mcp", &session).await
}

#[r2e_core::test]
async fn input_schema_reflects_the_params_type() {
    let list = listed().await;
    let add = tool(&list, "add");
    let schema = &add["inputSchema"];

    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["a"]["description"], "Left operand.");
    assert_eq!(schema["properties"]["b"]["description"], "Right operand.");
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(required.contains(&"a") && required.contains(&"b"), "{schema}");
    // The schemars `$schema` marker is stripped from the wire form.
    assert!(schema.get("$schema").is_none(), "{schema}");
}

#[r2e_core::test]
async fn output_schema_only_for_json_returns() {
    let list = listed().await;

    // Json<CalcResult> → outputSchema advertised.
    let add = tool(&list, "add");
    assert_eq!(
        add["outputSchema"]["properties"]["value"]["description"],
        "The result of the operation."
    );
    // Result<Json<T>, McpError> unwraps to the same T.
    let div = tool(&list, "div");
    assert!(div["outputSchema"]["properties"]["value"].is_object());

    // String / () returns advertise no outputSchema.
    for name in ["echo_id", "locked", "rich"] {
        assert!(
            tool(&list, name).get("outputSchema").is_none(),
            "`{name}` must not advertise an outputSchema"
        );
    }
}

#[r2e_core::test]
async fn parameterless_tools_get_an_empty_object_schema() {
    let list = listed().await;
    let locked = tool(&list, "locked");
    assert_eq!(locked["inputSchema"]["type"], "object");
    assert!(
        locked["inputSchema"].get("required").is_none()
            || locked["inputSchema"]["required"].as_array().unwrap().is_empty(),
        "{list}"
    );
}

#[r2e_core::test]
async fn nested_and_enum_types_keep_defs_inline() {
    let list = listed().await;
    let rich = tool(&list, "rich");
    let schema = &rich["inputSchema"];

    // Optional field is not required.
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(required.contains(&"name"), "{schema}");
    assert!(!required.contains(&"count"), "Option<T> must not be required: {schema}");

    // Nested struct and enum land in same-document `$defs` (kept inline —
    // draft 2020-12 refs, resolved by clients).
    assert_eq!(schema["properties"]["name"]["description"], "A documented required field.");
    let defs = schema["$defs"].as_object().expect("nested types produce $defs");
    assert!(defs.contains_key("Inner"), "{schema}");
    let mode = &defs["Mode"];
    let variants = mode["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("enum variants missing: {mode}"));
    assert!(variants.iter().any(|v| v == "Fast"), "{mode}");
    assert!(variants.iter().any(|v| v == "Thorough"), "{mode}");
}

#[r2e_core::test]
async fn descriptions_are_verbatim_doc_text() {
    let list = listed().await;

    // Single-line doc.
    assert_eq!(tool(&list, "add")["description"], "Add two numbers.");
    // Multi-paragraph doc: NO summary/body split (unlike OpenAPI) — the
    // paragraph break survives verbatim.
    assert_eq!(
        tool(&list, "div")["description"],
        "Divide `a` by `b`.\n\nFails with a domain error when `b` is zero."
    );
}

#[r2e_core::test]
async fn annotations_carry_the_tool_hints() {
    let list = listed().await;

    let add = tool(&list, "add");
    assert_eq!(add["annotations"]["readOnlyHint"], true);
    assert_eq!(add["annotations"]["idempotentHint"], true);

    // No hints declared → no annotations object at all.
    assert!(tool(&list, "echo_id").get("annotations").is_none());
}

#[r2e_core::test]
async fn tool_names_honor_the_name_override() {
    let list = listed().await;
    // `divide` is renamed via #[tool(name = "div")].
    assert!(list["tools"].as_array().unwrap().iter().all(|t| t["name"] != "divide"));
    tool(&list, "div");
}
