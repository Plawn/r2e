use r2e_core::http::{Body, HeaderMap, Router, Request, StatusCode};
use r2e_core::http::body::to_bytes;
use r2e_static::{EmbeddedFrontend, rust_embed};
use tower::ServiceExt;

#[derive(rust_embed::Embed, Clone)]
#[folder = "tests/fixtures"]
struct TestAssets;

async fn get(app: Router, path: &str) -> (StatusCode, HeaderMap, String) {
    get_with_headers(app, path, &[]).await
}

async fn get_with_headers(
    app: Router,
    path: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder().uri(path);
    for &(name, value) in headers {
        builder = builder.header(name, value);
    }
    let req = builder.body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

fn make_app(frontend: EmbeddedFrontend) -> Router {
    r2e_core::AppBuilder::new()
        .with_state(())
        .with(frontend)
        .build()
}

// ── Existing behavior ──────────────────────────────────────────────────────

#[r2e_core::test]
async fn exact_file_match_returns_content_and_mime() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (status, headers, body) = get(app, "/style.css").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("body { margin: 0; }"));
    assert_eq!(headers.get("content-type").unwrap(), "text/css");
}

#[r2e_core::test]
async fn root_serves_index_html() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (status, _, body) = get(app, "/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<h1>Hello</h1>"));
}

#[r2e_core::test]
async fn excluded_prefix_returns_404() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (status, _, _) = get(app, "/api/users").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[r2e_core::test]
async fn spa_fallback_returns_index_html() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (status, _, body) = get(app, "/some/unknown/route").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<h1>Hello</h1>"));
}

#[r2e_core::test]
async fn spa_disabled_returns_404_for_unknown() {
    let app = make_app(
        EmbeddedFrontend::builder::<TestAssets>()
            .spa_fallback(false)
            .build(),
    );
    let (status, _, _) = get(app, "/some/unknown/route").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[r2e_core::test]
async fn immutable_prefix_sets_cache_control() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (status, headers, _) = get(app, "/assets/app.abc123.js").await;

    assert_eq!(status, StatusCode::OK);
    let cc = headers.get("cache-control").unwrap().to_str().unwrap();
    assert!(cc.contains("immutable"));
    assert!(cc.contains("max-age=31536000"));
}

#[r2e_core::test]
async fn non_immutable_file_gets_default_cache_control() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (_, headers, _) = get(app, "/style.css").await;

    let cc = headers.get("cache-control").unwrap().to_str().unwrap();
    assert!(cc.contains("max-age=3600"));
    assert!(!cc.contains("immutable"));
}

#[r2e_core::test]
async fn etag_header_present() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (_, headers, _) = get(app, "/style.css").await;

    let etag = headers.get("etag").unwrap().to_str().unwrap();
    assert!(etag.starts_with('"') && etag.ends_with('"'));
}

#[r2e_core::test]
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

#[r2e_core::test]
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

#[r2e_core::test]
async fn js_file_has_correct_mime() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());
    let (_, headers, _) = get(app, "/assets/app.abc123.js").await;

    let ct = headers.get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("javascript"));
}

// ── #1: 304 Not Modified ───────────────────────────────────────────────────

#[r2e_core::test]
async fn if_none_match_returns_304_on_matching_etag() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());

    // First request to get the ETag.
    let (_, headers, _) = get(app.clone(), "/style.css").await;
    let etag = headers.get("etag").unwrap().to_str().unwrap();

    // Second request with If-None-Match.
    let (status, resp_headers, body) = get_with_headers(
        app,
        "/style.css",
        &[("If-None-Match", etag)],
    )
    .await;

    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());
    assert_eq!(
        resp_headers.get("etag").unwrap().to_str().unwrap(),
        etag,
    );
}

#[r2e_core::test]
async fn if_none_match_returns_200_on_mismatched_etag() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());

    let (status, _, body) = get_with_headers(
        app,
        "/style.css",
        &[("If-None-Match", "\"wrong-etag\"")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("body { margin: 0; }"));
}

#[r2e_core::test]
async fn if_none_match_star_returns_304() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());

    let (status, _, body) =
        get_with_headers(app, "/style.css", &[("If-None-Match", "*")]).await;

    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());
}

#[r2e_core::test]
async fn if_none_match_multiple_etags() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());

    let (_, headers, _) = get(app.clone(), "/style.css").await;
    let etag = headers.get("etag").unwrap().to_str().unwrap();

    let multi = format!("\"other\", {}", etag);
    let (status, _, _) = get_with_headers(
        app,
        "/style.css",
        &[("If-None-Match", &multi)],
    )
    .await;

    assert_eq!(status, StatusCode::NOT_MODIFIED);
}

// ── #2: Compression ────────────────────────────────────────────────────────

#[r2e_core::test]
async fn brotli_served_when_accepted() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());

    let (status, headers, _) = get_with_headers(
        app,
        "/style.css",
        &[("Accept-Encoding", "br, gzip")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("content-encoding").unwrap().to_str().unwrap(),
        "br",
    );
    assert_eq!(
        headers.get("content-type").unwrap().to_str().unwrap(),
        "text/css",
    );
    assert_eq!(
        headers.get("vary").unwrap().to_str().unwrap(),
        "Accept-Encoding",
    );
}

#[r2e_core::test]
async fn gzip_served_when_br_not_accepted() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());

    let (status, headers, _) = get_with_headers(
        app,
        "/style.css",
        &[("Accept-Encoding", "gzip")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("content-encoding").unwrap().to_str().unwrap(),
        "gzip",
    );
}

#[r2e_core::test]
async fn uncompressed_when_no_compressed_variant_exists() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());

    // index.html has no .br/.gz variant
    let (status, headers, body) = get_with_headers(
        app,
        "/index.html",
        &[("Accept-Encoding", "br, gzip")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("content-encoding").is_none());
    assert!(body.contains("<h1>Hello</h1>"));
}

#[r2e_core::test]
async fn compression_disabled_serves_uncompressed() {
    let app = make_app(
        EmbeddedFrontend::builder::<TestAssets>()
            .compression(false)
            .build(),
    );

    let (status, headers, body) = get_with_headers(
        app,
        "/style.css",
        &[("Accept-Encoding", "br, gzip")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("content-encoding").is_none());
    assert!(body.contains("body { margin: 0; }"));
}

#[r2e_core::test]
async fn quality_zero_encoding_rejected() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());

    let (_, headers, _) = get_with_headers(
        app,
        "/style.css",
        &[("Accept-Encoding", "br;q=0, gzip")],
    )
    .await;

    assert_eq!(
        headers.get("content-encoding").unwrap().to_str().unwrap(),
        "gzip",
    );
}

// ── #3: SPA fallback cache control ─────────────────────────────────────────

#[r2e_core::test]
async fn spa_fallback_uses_no_cache() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());

    let (_, headers, _) = get(app, "/some/unknown/route").await;
    let cc = headers.get("cache-control").unwrap().to_str().unwrap();

    assert_eq!(cc, "no-cache");
}

#[r2e_core::test]
async fn direct_index_uses_default_cache() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());

    let (_, headers, _) = get(app, "/index.html").await;
    let cc = headers.get("cache-control").unwrap().to_str().unwrap();

    assert!(cc.contains("max-age=3600"));
}

#[r2e_core::test]
async fn custom_fallback_cache_control() {
    let app = make_app(
        EmbeddedFrontend::builder::<TestAssets>()
            .fallback_cache_control("no-store")
            .build(),
    );

    let (_, headers, _) = get(app, "/some/route").await;
    let cc = headers.get("cache-control").unwrap().to_str().unwrap();

    assert_eq!(cc, "no-store");
}

// ── #4: Content-Length ─────────────────────────────────────────────────────

#[r2e_core::test]
async fn content_length_present() {
    let app = make_app(
        EmbeddedFrontend::builder::<TestAssets>()
            .compression(false)
            .build(),
    );

    let (_, headers, body) = get(app, "/style.css").await;
    let cl: usize = headers
        .get("content-length")
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();

    assert_eq!(cl, body.len());
    assert!(cl > 0);
}

#[r2e_core::test]
async fn content_length_on_compressed_response() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());

    let (_, headers, _) = get_with_headers(
        app,
        "/style.css",
        &[("Accept-Encoding", "gzip")],
    )
    .await;

    assert!(headers.get("content-length").is_some());
}

// ── #5: (hex encoding is internal — covered by ETag tests above) ──────────

// ── #6: Range requests ─────────────────────────────────────────────────────

#[r2e_core::test]
async fn range_request_returns_206() {
    let app = make_app(
        EmbeddedFrontend::builder::<TestAssets>()
            .compression(false)
            .build(),
    );

    let (status, headers, body) = get_with_headers(
        app,
        "/style.css",
        &[("Range", "bytes=0-3")],
    )
    .await;

    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, "body");
    assert_eq!(
        headers.get("content-length").unwrap().to_str().unwrap(),
        "4",
    );
    let cr = headers.get("content-range").unwrap().to_str().unwrap();
    assert!(cr.starts_with("bytes 0-3/"));
}

#[r2e_core::test]
async fn range_suffix_request() {
    let app = make_app(
        EmbeddedFrontend::builder::<TestAssets>()
            .compression(false)
            .build(),
    );

    // "body { margin: 0; }\n" — last 4 bytes = "}\n" ... let's request last 3
    let (status, headers, body) = get_with_headers(
        app,
        "/style.css",
        &[("Range", "bytes=-3")],
    )
    .await;

    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body.len(), 3);
    assert!(headers.get("content-range").is_some());
}

#[r2e_core::test]
async fn range_open_end() {
    let app = make_app(
        EmbeddedFrontend::builder::<TestAssets>()
            .compression(false)
            .build(),
    );

    // Request from byte 5 to end: "{ margin: 0; }\n"
    let (status, _, body) = get_with_headers(
        app,
        "/style.css",
        &[("Range", "bytes=5-")],
    )
    .await;

    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert!(body.starts_with("{ margin"));
}

#[r2e_core::test]
async fn invalid_range_returns_416() {
    let app = make_app(
        EmbeddedFrontend::builder::<TestAssets>()
            .compression(false)
            .build(),
    );

    let (status, headers, _) = get_with_headers(
        app,
        "/style.css",
        &[("Range", "bytes=9999-")],
    )
    .await;

    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
    let cr = headers.get("content-range").unwrap().to_str().unwrap();
    assert!(cr.starts_with("bytes */"));
}

#[r2e_core::test]
async fn accept_ranges_header_present() {
    let app = make_app(
        EmbeddedFrontend::builder::<TestAssets>()
            .compression(false)
            .build(),
    );

    let (_, headers, _) = get(app, "/style.css").await;
    assert_eq!(
        headers.get("accept-ranges").unwrap().to_str().unwrap(),
        "bytes",
    );
}

#[r2e_core::test]
async fn no_accept_ranges_on_compressed() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());

    let (_, headers, _) = get_with_headers(
        app,
        "/style.css",
        &[("Accept-Encoding", "br")],
    )
    .await;

    assert!(headers.get("accept-ranges").is_none());
}

#[r2e_core::test]
async fn range_ignored_for_compressed_response() {
    let app = make_app(EmbeddedFrontend::new::<TestAssets>());

    let (status, headers, _) = get_with_headers(
        app,
        "/style.css",
        &[("Accept-Encoding", "br"), ("Range", "bytes=0-3")],
    )
    .await;

    // Should be 200 (full compressed), not 206
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("content-encoding").unwrap().to_str().unwrap(),
        "br",
    );
}
