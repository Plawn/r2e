use http::Request;
use http_body_util::BodyExt;
use r2e_core::http::body::Body;
use r2e_core::http::Router;
use r2e_core::meta::RouteInfo;
use r2e_openapi::{openapi_routes, OpenApiConfig};
use serde_json::Value;
use tower::ServiceExt;

// ── Helpers ─────────────────────────────────────────────────────────────────

fn simple_route(method: &str, path: &str, op: &str) -> RouteInfo {
    RouteInfo {
        path: path.to_string(),
        method: method.to_string(),
        operation_id: op.to_string(),
        summary: None,
        request_body_type: None,
        request_body_schema: None,
        response_type: None,
        params: vec![],
        roles: vec![],
        tag: None,
    }
}

fn config_with_ui() -> OpenApiConfig {
    OpenApiConfig::new("Test API", "1.0.0").with_docs_ui(true)
}

fn config_without_ui() -> OpenApiConfig {
    OpenApiConfig::new("Test API", "1.0.0").with_docs_ui(false)
}

async fn get_response(
    router: Router,
    path: &str,
) -> (http::StatusCode, String, http::HeaderMap) {
    let req = Request::builder()
        .uri(path)
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(req).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    (status, body_str, headers)
}

// ── Phase 6: Plugin & Routes Integration ────────────────────────────────────

#[tokio::test]
async fn openapi_json_endpoint() {
    let routes = vec![simple_route("GET", "/users", "list_users")];
    let router = openapi_routes::<()>(config_with_ui(), &routes);

    let (status, body, _) = get_response(router, "/openapi.json").await;
    assert_eq!(status, http::StatusCode::OK);

    let spec: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(spec["openapi"], "3.0.3");
    assert!(spec["paths"]["/users"]["get"].is_object());
}

#[tokio::test]
async fn openapi_json_content_type() {
    let routes = vec![simple_route("GET", "/health", "health")];
    let router = openapi_routes::<()>(config_with_ui(), &routes);

    let (_, _, headers) = get_response(router, "/openapi.json").await;
    assert_eq!(
        headers.get("content-type").unwrap().to_str().unwrap(),
        "application/json"
    );
}

#[tokio::test]
async fn docs_ui_when_enabled() {
    let routes = vec![];
    let router = openapi_routes::<()>(config_with_ui(), &routes);

    let (status, body, _) = get_response(router, "/docs").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(body.contains("<html"));
    assert!(body.contains("wti-element"));
    assert!(body.contains("spec-url=\"/openapi.json\""));
}

#[tokio::test]
async fn docs_ui_when_disabled() {
    let routes = vec![];
    let router = openapi_routes::<()>(config_without_ui(), &routes);

    let (status, _, _) = get_response(router, "/docs").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn docs_css_served() {
    let routes = vec![];
    let router = openapi_routes::<()>(config_with_ui(), &routes);

    let (status, body, headers) = get_response(router, "/docs/wti-element.css").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(
        headers.get("content-type").unwrap().to_str().unwrap(),
        "text/css"
    );
    assert!(!body.is_empty());
}

#[tokio::test]
async fn docs_js_served() {
    let routes = vec![];
    let router = openapi_routes::<()>(config_with_ui(), &routes);

    let (status, body, headers) = get_response(router, "/docs/wti-element.js").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(
        headers.get("content-type").unwrap().to_str().unwrap(),
        "application/javascript"
    );
    assert!(!body.is_empty());
}

#[tokio::test]
async fn spec_includes_registered_routes() {
    let routes = vec![
        simple_route("GET", "/users", "list_users"),
        simple_route("POST", "/users", "create_user"),
        simple_route("GET", "/roles", "list_roles"),
    ];
    let router = openapi_routes::<()>(config_with_ui(), &routes);

    let (_, body, _) = get_response(router, "/openapi.json").await;
    let spec: Value = serde_json::from_str(&body).unwrap();

    let paths = spec["paths"].as_object().unwrap();
    assert!(paths.contains_key("/users"));
    assert!(paths.contains_key("/roles"));
    assert!(spec["paths"]["/users"]["get"].is_object());
    assert!(spec["paths"]["/users"]["post"].is_object());
}

#[tokio::test]
async fn docs_assets_not_served_when_disabled() {
    let routes = vec![];
    let router = openapi_routes::<()>(config_without_ui(), &routes);

    let req_css = Request::builder()
        .uri("/docs/wti-element.css")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req_css).await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::NOT_FOUND);

    let req_js = Request::builder()
        .uri("/docs/wti-element.js")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req_js).await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn openapi_json_always_served_regardless_of_docs_ui() {
    let routes = vec![simple_route("GET", "/test", "test")];
    let router = openapi_routes::<()>(config_without_ui(), &routes);

    let (status, body, _) = get_response(router, "/openapi.json").await;
    assert_eq!(status, http::StatusCode::OK);
    let spec: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(spec["openapi"], "3.0.3");
}
