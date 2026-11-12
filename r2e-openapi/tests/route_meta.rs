//! The routes macro and the spec builder agree with the runtime (Tasker #1272).
//!
//! Drives the real codegen through `build_spec_with_warnings`: a tuple
//! `Path<(A, B)>` documents one parameter per placeholder, and a verb's
//! success status is the one the handler answers — so a `DELETE` returning
//! `Json<T>` is documented at 200 **with its body**, where the former 204
//! default silently dropped it.

use r2e_core::controller::Controller;
use r2e_core::di::meta::{MetaRegistry, RouteInfo};
use r2e_core::http::extract::Path;
use r2e_core::http::{Json, StatusCode};
use r2e_core::type_list::HNil;
use r2e_macros::{controller, routes};
use r2e_openapi::{build_spec_with_warnings, OpenApiConfig};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Serialize, JsonSchema)]
struct Member {
    id: u64,
}

#[controller(path = "/workspaces")]
struct MembersController {}

#[routes]
impl MembersController {
    #[get("/{ws}/members/{user}")]
    async fn get(&self, Path((ws, user)): Path<(String, u64)>) -> Json<Member> {
        let _ = ws;
        Json(Member { id: user })
    }

    #[post("/{ws}/members")]
    async fn add(&self, Path(ws): Path<String>) -> Json<Member> {
        let _ = ws;
        Json(Member { id: 1 })
    }

    #[delete("/{ws}/members/{user}")]
    async fn remove(&self, Path((ws, user)): Path<(String, u64)>) -> Json<Member> {
        let _ = ws;
        Json(Member { id: user })
    }

    #[delete("/{ws}")]
    #[status(204)]
    async fn purge(&self, Path(ws): Path<String>) -> StatusCode {
        let _ = ws;
        StatusCode::NO_CONTENT
    }
}

fn spec() -> Value {
    let mut registry = MetaRegistry::new();
    <MembersController as Controller<HNil, _>>::register_meta(&mut registry);
    let routes = registry.take::<RouteInfo>();
    let (spec, warnings) = build_spec_with_warnings(&OpenApiConfig::new("t", "1"), &routes);
    assert!(warnings.is_empty(), "{warnings:?}");
    spec
}

fn statuses(responses: &Value) -> Vec<&str> {
    let mut all: Vec<&str> = responses
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .filter(|s| s.starts_with('2'))
        .collect();
    all.sort_unstable();
    all
}

#[test]
fn tuple_path_documents_one_parameter_per_placeholder() {
    let spec = spec();
    let params = &spec["paths"]["/workspaces/{ws}/members/{user}"]["get"]["parameters"];

    assert_eq!(
        params,
        &json!([
            { "name": "ws", "in": "path", "required": true, "schema": { "type": "string" } },
            { "name": "user", "in": "path", "required": true, "schema": { "type": "integer" } },
        ])
    );
}

#[test]
fn post_is_documented_at_the_status_it_answers() {
    let spec = spec();
    let responses = &spec["paths"]["/workspaces/{ws}/members"]["post"]["responses"];

    assert_eq!(statuses(responses), ["200"]);
    assert_eq!(
        responses["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/Member"
    );
}

#[test]
fn delete_returning_json_is_documented_with_its_body() {
    let spec = spec();
    let responses = &spec["paths"]["/workspaces/{ws}/members/{user}"]["delete"]["responses"];

    assert_eq!(statuses(responses), ["200"]);
    assert_eq!(
        responses["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/Member"
    );
}

#[test]
fn declared_204_stays_bodyless() {
    let spec = spec();
    let responses = &spec["paths"]["/workspaces/{ws}"]["delete"]["responses"];

    assert_eq!(statuses(responses), ["204"]);
    assert!(responses["204"].get("content").is_none());
}
