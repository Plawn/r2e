//! `/openapi.json` serves a document that is immutable for the app's lifetime.
//!
//! The handler must hand out a refcount bump (`Bytes`), not a fresh copy of the
//! whole spec: on a real app the rendered document is tens to hundreds of kB,
//! and a `String` clone memcpy'd all of it on every hit.

use r2e::di::meta::RouteInfo;
use r2e::http::{Body, Request, Router, StatusCode};
use r2e::r2e_openapi::{openapi_routes, OpenApiConfig};
use tower::ServiceExt;

use crate::counter::{assert_config_size_invariant, runtime, steady_state, Alloc};

const ITERATIONS: u64 = 100;

fn route(i: usize) -> RouteInfo {
    RouteInfo {
        path: format!("/generated/resource-{i}/{{id}}"),
        method: "GET".into(),
        operation_id: format!("generated_operation_{i}"),
        summary: Some(format!("Generated operation {i}")),
        description: Some(format!(
            "A generated route used to inflate the rendered OpenAPI document ({i})"
        )),
        request_body_type: None,
        request_body_schema: None,
        request_body_content_type: None,
        request_body_required: false,
        response_type: None,
        response_schema: None,
        response_status: 200,
        response_unmapped: None,
        params: Vec::new(),
        roles: Vec::new(),
        tag: Some("generated".into()),
        deprecated: false,
        has_auth: false,
    }
}

fn spec_router(routes: usize) -> Router {
    let routes: Vec<RouteInfo> = (0..routes).map(route).collect();
    openapi_routes::<()>(
        OpenApiConfig::new("hot-path guard", "0.0.0").with_docs_ui(false),
        &routes,
    )
}

fn drive(rt: &r2e::rt::Runtime, router: &Router) -> Alloc {
    steady_state(ITERATIONS, || {
        let response = rt
            .block_on(
                router.clone().oneshot(
                    Request::builder()
                        .uri("/openapi.json")
                        .body(Body::empty())
                        .expect("request"),
                ),
            )
            .expect("infallible router");
        assert_eq!(response.status(), StatusCode::OK);
    })
}

/// Serving the spec must cost the same whether the document describes 2 routes
/// or 200 — the body is pre-encoded once at router build time.
#[test]
fn openapi_document_is_not_recopied_per_request() {
    let rt = runtime();

    let small = drive(&rt, &spec_router(2));
    let large = drive(&rt, &spec_router(200));

    // 200 generated routes render to well over 100 kB of JSON, so a per-request
    // copy of the document would blow past this slack immediately.
    assert_config_size_invariant("GET /openapi.json", small, large, 4, 4_096);
}
