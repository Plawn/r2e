//! A `#[derive(Params)]` struct as the last handler argument is not a request
//! body (#1170).
//!
//! The last extracted parameter of a route sits in body position, so the
//! routes macro used to flag a Params struct there as an unmapped body and
//! `build_spec` logged a `MissingRequestBody` warning at every boot. A Params
//! struct is a `FromRequestParts` extractor (path + query) whose fields are
//! already published as parameters; this drives the real codegen end to end
//! through `spec_warnings`.

use r2e_core::controller::Controller;
use r2e_core::di::meta::{MetaRegistry, RouteInfo};
use r2e_core::http::extract::FromRequest;
use r2e_core::http::{Request, Response};
use r2e_core::type_list::HNil;
use r2e_macros::{controller, routes, Params};
use r2e_openapi::{build_spec, spec_warnings, OpenApiConfig, SchemaGap};
use serde::Deserialize;

#[derive(Debug, Default, Params, Deserialize)]
pub struct JobListParams {
    pub kind: Option<String>,
    pub limit: Option<i64>,
}

/// A real body extractor without `RequestBodySchema`: still warned about.
pub struct Opaque;

impl<S: Send + Sync> FromRequest<S> for Opaque {
    type Rejection = Response;

    async fn from_request(_req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self)
    }
}

#[controller(path = "/api/admin")]
pub struct JobsController {}

#[routes]
impl JobsController {
    #[get("/jobs")]
    async fn list(&self, _params: JobListParams) -> String {
        String::new()
    }

    #[post("/enrich/backfill")]
    async fn backfill(&self, _params: JobListParams) -> String {
        String::new()
    }

    #[post("/opaque")]
    async fn opaque(&self, _body: Opaque) -> String {
        String::new()
    }
}

fn routes_meta() -> Vec<RouteInfo> {
    let mut registry = MetaRegistry::new();
    <JobsController as Controller<HNil, _>>::register_meta(&mut registry);
    registry.take::<RouteInfo>()
}

fn route<'a>(routes: &'a [RouteInfo], method: &str, path: &str) -> &'a RouteInfo {
    routes
        .iter()
        .find(|r| r.method == method && r.path == path)
        .unwrap_or_else(|| panic!("no route {method} {path}"))
}

#[test]
fn params_struct_last_is_not_a_request_body() {
    let routes = routes_meta();
    for (method, path) in [
        ("GET", "/api/admin/jobs"),
        ("POST", "/api/admin/enrich/backfill"),
    ] {
        let r = route(&routes, method, path);
        assert!(r.request_body.is_none(), "{method} {path}");
        assert!(r.request_body_unmapped.is_none(), "{method} {path}");
        let names: Vec<&str> = r.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["kind", "limit"], "{method} {path}");
    }
}

#[test]
fn only_the_real_body_extractor_warns() {
    let warnings = spec_warnings(&routes_meta());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].method, "POST");
    assert_eq!(warnings[0].path, "/api/admin/opaque");
    assert_eq!(
        warnings[0].gap,
        SchemaGap::MissingRequestBody {
            type_name: "Opaque".to_string()
        }
    );
}

#[test]
fn params_are_documented_as_query_parameters() {
    let spec = build_spec(&OpenApiConfig::new("t", "1"), &routes_meta());
    for (verb, path) in [
        ("get", "/api/admin/jobs"),
        ("post", "/api/admin/enrich/backfill"),
    ] {
        let op = &spec["paths"][path][verb];
        assert!(op.get("requestBody").is_none(), "{verb} {path}: {op}");
        let names: Vec<&str> = op["parameters"]
            .as_array()
            .expect("parameters")
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["kind", "limit"], "{verb} {path}");
    }
}
