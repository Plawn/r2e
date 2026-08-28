//! Typed tool parameters.
//!
//! `#[tool]` methods declare their arguments as one `Params<T>` parameter
//! where `T: Deserialize + JsonSchema + ObjectParams`. The explicit marker
//! makes scalar, tuple and enum roots a compile error instead of silently
//! advertising an unconstrained object schema.

use schemars::generate::SchemaSettings;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::McpError;
use crate::route::SchemaObject;

/// Wrapper marking a tool method parameter as the deserialized `arguments`
/// object.
///
/// ```ignore
/// #[tool(name = "add")]
/// async fn add(&self, Params(p): Params<AddIn>) -> Json<AddOut> { … }
/// ```
#[derive(Debug, Clone)]
pub struct Params<T>(pub T);

/// Marker for named structs that are valid MCP argument objects.
///
/// Derive it with `#[derive(ObjectParams)]`. The derive rejects enums, tuple
/// structs and unit structs, so a `Params<u32>`/`Params<MyEnum>` call cannot
/// compile into an invalid MCP `inputSchema`.
pub trait ObjectParams: private::Sealed {}

#[doc(hidden)]
pub mod private {
    /// Implementation detail for the `ObjectParams` derive.
    pub trait Sealed {}
}

/// Schema + deserialization contract used by the generated dispatch code.
///
/// Blanket-implemented for every
/// `T: DeserializeOwned + JsonSchema + ObjectParams`; a missing derive
/// therefore surfaces as a plain trait-bound error at the tool method.
pub trait ToolParams: Sized {
    /// The JSON Schema (draft 2020-12) object describing the arguments.
    fn input_schema() -> SchemaObject;

    /// Deserialize the raw `arguments` object.
    fn from_arguments(arguments: Value) -> Result<Self, McpError>;
}

impl<T: DeserializeOwned + JsonSchema + ObjectParams> ToolParams for T {
    fn input_schema() -> SchemaObject {
        schema_object_for::<T>()
    }

    fn from_arguments(arguments: Value) -> Result<Self, McpError> {
        serde_json::from_value(arguments)
            .map_err(|e| McpError::InvalidParams(format!("invalid tool arguments: {e}")))
    }
}

/// Generate the draft 2020-12 root schema for `T` as a plain JSON object,
/// with the `$schema` marker stripped (MCP `inputSchema` is a bare object;
/// the dialect is fixed by the spec). `$defs` are KEPT inline —
/// same-document refs are resolved by rmcp's input validator and all
/// mainstream clients.
///
/// [`ObjectParams`] guarantees that MCP input types originate from named
/// structs. The runtime check remains as a defensive backstop for custom
/// `JsonSchema` implementations that deliberately emit a non-object root.
pub fn schema_object_for<T: ?Sized + JsonSchema>() -> SchemaObject {
    let schema = SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<T>();
    match schema.to_value() {
        Value::Object(mut map) => {
            map.remove("$schema");
            let is_object_schema = match map.get("type") {
                Some(Value::String(t)) => t == "object",
                // No `type` at all (e.g. an empty schema / `Value` params):
                // accept — it still validates objects.
                None => true,
                _ => false,
            };
            if !is_object_schema {
                tracing::warn!(
                    type_name = std::any::type_name::<T>(),
                    "tool parameter type does not produce an object schema; \
                     advertising an unconstrained object inputSchema"
                );
                return empty_object_schema();
            }
            ensure_object_type(map)
        }
        _ => {
            tracing::warn!(
                type_name = std::any::type_name::<T>(),
                "tool parameter type produced a non-object root schema; \
                 advertising an unconstrained object inputSchema"
            );
            empty_object_schema()
        }
    }
}

/// `{"type": "object"}` — the unconstrained accept-all input schema.
pub fn empty_object_schema() -> SchemaObject {
    let mut map = SchemaObject::new();
    map.insert("type".to_string(), Value::String("object".to_string()));
    map
}

fn ensure_object_type(mut map: SchemaObject) -> SchemaObject {
    map.entry("type".to_string())
        .or_insert_with(|| Value::String("object".to_string()));
    map
}

/// Derive a prompt's declared arguments from its `Params<T>` object schema:
/// one entry per `properties` key, `required` from the schema's `required`
/// array, `title`/`description` carried over from the property schema
/// (schemars emits doc comments as `description`).
///
/// Prompt argument values are strings per the MCP spec — the schema still
/// drives deserialization, but only the property names, requiredness and
/// descriptions are advertised.
pub fn prompt_arguments_from_schema(schema: &SchemaObject) -> Vec<crate::route::PromptArgumentDef> {
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    properties
        .iter()
        .map(|(name, prop)| {
            let field = |key: &str| {
                prop.as_object()
                    .and_then(|o| o.get(key))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            crate::route::PromptArgumentDef {
                name: name.clone(),
                title: field("title"),
                description: field("description"),
                required: required.contains(&name.as_str()),
            }
        })
        .collect()
}
