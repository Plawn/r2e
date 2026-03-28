use r2e_core::http::{Body, HeaderMap, Router, Request, StatusCode};
use r2e_core::http::body::to_bytes;
use r2e_static::{EmbeddedFrontend, rust_embed};
use tower::ServiceExt;

#[derive(rust_embed::Embed, Clone)]
#[folder = "tests/fixtures"]
struct TestAssets;

async fn get(app: Router, path: &str) -> (StatusCode, HeaderMap, String) {
    let req = Request::builder()
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

fn make_app(frontend: EmbeddedFrontend) -> Router {
    r2e_core::AppBuilder::new()
        .with_state(())
        .with(frontend)
        .build()
}

#[tokio::test]
async fn exact_file_match_returns_content_and_mime() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (status, headers, body) = get(app, "/style.css").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("body { margin: 0; }"));
    assert_eq!(headers.get("content-type").unwrap(), "text/css");
}

#[tokio::test]
async fn root_serves_index_html() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (status, _, body) = get(app, "/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<h1>Hello</h1>"));
}

#[tokio::test]
async fn excluded_prefix_returns_404() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (status, _, _) = get(app, "/api/users").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn spa_fallback_returns_index_html() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (status, _, body) = get(app, "/some/unknown/route").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<h1>Hello</h1>"));
}

#[tokio::test]
async fn spa_disabled_returns_404_for_unknown() {
    let app = make_app(
        EmbeddedFrontend::builder::<TestAssets>()
            .spa_fallback(false)
            .build(),
    );
    let (status, _, _) = get(app, "/some/unknown/route").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn immutable_prefix_sets_cache_control() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (status, headers, _) = get(app, "/assets/app.abc123.js").await;

    assert_eq!(status, StatusCode::OK);
    let cc = headers.get("cache-control").unwrap().to_str().unwrap();
    assert!(cc.contains("immutable"));
    assert!(cc.contains("max-age=31536000"));
}

#[tokio::test]
async fn non_immutable_file_gets_default_cache_control() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (_, headers, _) = get(app, "/style.css").await;

    let cc = headers.get("cache-control").unwrap().to_str().unwrap();
    assert!(cc.contains("max-age=3600"));
    assert!(!cc.contains("immutable"));
}

#[tokio::test]
async fn etag_header_present() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (_, headers, _) = get(app, "/style.css").await;

    let etag = headers.get("etag").unwrap().to_str().unwrap();
    assert!(etag.starts_with('"') && etag.ends_with('"'));
}

#[tokio::test]
async fn base_path_routing() {
    let app = make_app(
        EmbeddedFrontend::builder::<TestAssets>()
            .base_path("/docs")
            .spa_fallback(false)
            .build(),
    );

    let (status, _, body) = get(app.clone(), "/docs/style.css").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("body { margin: 0; }"));

    let (status, _, body) = get(app.clone(), "/docs/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<h1>Hello</h1>"));

    let (status, _, _) = get(app, "/other/style.css").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn custom_excluded_prefix() {
    let app = make_app(
        EmbeddedFrontend::builder::<TestAssets>()
            .exclude_prefix("graphql/")
            .build(),
    );

    let (status, _, _) = get(app.clone(), "/api/test").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, _) = get(app, "/graphql/query").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn js_file_has_correct_mime() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (_, headers, _) = get(app, "/assets/app.abc123.js").await;

    let ct = headers.get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("javascript"));
}
