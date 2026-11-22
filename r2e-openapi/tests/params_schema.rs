//! A parameter's OpenAPI schema comes from its type (Tasker #1441).
//!
//! `#[derive(Params)]` used to guess the schema from the type's *name*:
//! primitives by name, everything else `string`. A bounded newtype or an enum
//! that implements `schemars::JsonSchema` was therefore published as a bare
//! `string`, and the generated clients typed it as one. The schema is now the
//! type's own when it implements `JsonSchema`; a type that does not keeps the
//! name-based fallback, with no trait bound added to the fields.

use std::str::FromStr;

use r2e_core::controller::Controller;
use r2e_core::di::meta::{MetaRegistry, RouteInfo};
use r2e_core::http::extract::Path;
use r2e_core::type_list::HNil;
use r2e_macros::{controller, routes, Params};
use r2e_openapi::{build_spec, OpenApiConfig};
use schemars::JsonSchema;
use serde_json::{json, Value};

/// An integer clamped into `MIN..=MAX`, `DEFAULT` when absent — the consumer
/// shape that motivated the ticket. Its schema is hand-written.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct Clamped<const MIN: i64, const MAX: i64, const DEFAULT: i64>(i64);

impl<const MIN: i64, const MAX: i64, const DEFAULT: i64> Default for Clamped<MIN, MAX, DEFAULT> {
    fn default() -> Self {
        Self(DEFAULT)
    }
}

impl<const MIN: i64, const MAX: i64, const DEFAULT: i64> FromStr for Clamped<MIN, MAX, DEFAULT> {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<i64>().map(|v| Self(v.clamp(MIN, MAX)))
    }
}

impl<const MIN: i64, const MAX: i64, const DEFAULT: i64> JsonSchema
    for Clamped<MIN, MAX, DEFAULT>
{
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("Clamped_{MIN}_{MAX}_{DEFAULT}").into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "integer",
            "minimum": MIN,
            "maximum": MAX,
            "default": DEFAULT,
        })
    }
}

type PageLimit = Clamped<1, 100, 20>;

// A closed vocabulary. Not `inline_schema`: schemars would reference it
// through `$defs`, which a parameter schema cannot carry — it is inlined.
// (Plain comments: a doc comment would become the schema's `description`.)
#[derive(Debug, Clone, Copy, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProductSort {
    Newest,
    PriceAsc,
    PriceDesc,
}

impl FromStr for ProductSort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "newest" => Ok(Self::Newest),
            "price_asc" => Ok(Self::PriceAsc),
            "price_desc" => Ok(Self::PriceDesc),
            other => Err(format!("unknown sort `{other}`")),
        }
    }
}

/// `FromStr` only — no `JsonSchema`. Must keep compiling and keep its
/// name-based `string` schema.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Opaque(String);

impl FromStr for Opaque {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_owned()))
    }
}

#[derive(Debug, Params)]
pub struct Paging {
    #[param(default)]
    pub limit: PageLimit,
    pub offset: Option<u32>,
}

#[derive(Debug, Params)]
pub struct ProductListParams {
    #[param(path)]
    pub shop: ProductSort,
    pub sort: Option<ProductSort>,
    pub cursor: Option<Opaque>,
    pub token: Opaque,
    pub q: Option<String>,
    #[header("x-page-limit")]
    pub header_limit: Option<PageLimit>,
    #[params(prefix = "page")]
    pub paging: Paging,
}

#[controller(path = "/shops")]
pub struct ProductController {}

#[routes]
impl ProductController {
    #[get("/{shop}/products")]
    async fn list(&self, _params: ProductListParams) -> String {
        String::new()
    }

    /// A bare `Path<T>` types its placeholder the same way.
    #[get("/by-sort/{sort}/{opaque}")]
    async fn by_sort(&self, Path((_sort, _opaque)): Path<(ProductSort, String)>) -> String {
        String::new()
    }
}

fn routes_meta() -> Vec<RouteInfo> {
    let mut registry = MetaRegistry::new();
    <ProductController as Controller<HNil, _>>::register_meta(&mut registry);
    registry.take::<RouteInfo>()
}

fn parameters(path: &str) -> Value {
    let spec = build_spec(&OpenApiConfig::new("t", "0"), &routes_meta());
    spec["paths"][path]["get"]["parameters"].clone()
}

fn param<'a>(params: &'a Value, name: &str) -> &'a Value {
    params
        .as_array()
        .expect("parameters")
        .iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("no parameter `{name}` in {params}"))
}

const SORT_ENUM: [&str; 3] = ["newest", "price_asc", "price_desc"];

#[test]
fn params_struct_publishes_each_field_schema() {
    let params = parameters("/shops/{shop}/products");

    assert_eq!(
        params,
        json!([
            {
                "name": "shop",
                "in": "path",
                "required": true,
                "schema": { "type": "string", "enum": SORT_ENUM }
            },
            {
                "name": "sort",
                "in": "query",
                "required": false,
                "schema": { "type": "string", "enum": SORT_ENUM }
            },
            { "name": "cursor", "in": "query", "required": false, "schema": { "type": "string" } },
            { "name": "token", "in": "query", "required": true, "schema": { "type": "string" } },
            { "name": "q", "in": "query", "required": false, "schema": { "type": "string" } },
            {
                "name": "x-page-limit",
                "in": "header",
                "required": false,
                "schema": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
            },
            {
                "name": "page.limit",
                "in": "query",
                "required": false,
                "schema": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
            },
            {
                "name": "page.offset",
                "in": "query",
                "required": false,
                "schema": { "type": "integer", "format": "uint32", "minimum": 0 }
            },
        ])
    );
}

#[test]
fn option_publishes_the_inner_schema_without_null() {
    let params = parameters("/shops/{shop}/products");
    let sort = param(&params, "sort");

    assert_eq!(sort["required"], false);
    assert_eq!(sort["schema"], json!({ "type": "string", "enum": SORT_ENUM }));
    assert!(!sort["schema"].to_string().contains("null"));
}

#[test]
fn type_without_json_schema_keeps_the_name_based_fallback() {
    let routes = routes_meta();
    let route = routes
        .iter()
        .find(|r| r.path == "/shops/{shop}/products")
        .expect("route");
    let token = route.params.iter().find(|p| p.name == "token").expect("token");

    assert_eq!(token.param_type, "string");
    assert_eq!(token.schema, None);

    let params = parameters("/shops/{shop}/products");
    assert_eq!(param(&params, "token")["schema"], json!({ "type": "string" }));
}

#[test]
fn parameter_schemas_are_inline_and_leave_no_component() {
    let spec = build_spec(&OpenApiConfig::new("t", "0"), &routes_meta());

    let params = spec["paths"]["/shops/{shop}/products"]["get"]["parameters"].to_string();
    assert!(!params.contains("$ref"), "{params}");
    assert!(!params.contains("$defs"), "{params}");
    assert!(spec["components"]["schemas"]["ProductSort"].is_null());
}

#[test]
fn bare_path_extractor_publishes_the_type_schema() {
    let params = parameters("/shops/by-sort/{sort}/{opaque}");

    assert_eq!(
        params,
        json!([
            {
                "name": "sort",
                "in": "path",
                "required": true,
                "schema": { "type": "string", "enum": SORT_ENUM }
            },
            { "name": "opaque", "in": "path", "required": true, "schema": { "type": "string" } },
        ])
    );
}
