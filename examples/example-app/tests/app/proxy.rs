//! Proxy-shaped routing through the real app blueprint: `#[any]` wildcard
//! routes with raw-`Request` streaming pass-through, and the app-wide
//! `#[fallback]` (see `ProxyController`).

use r2e_test::TestApp;

#[r2e::test(app = example_app::ExampleApp)]
async fn any_route_requires_proxy_key(app: TestApp) {
    let resp = app.get("/proxy/some/artifact").send().await;
    resp.assert_unauthorized();
    assert_eq!(resp.header("www-authenticate"), Some("ProxyKey"));
}

#[r2e::test(app = example_app::ExampleApp)]
async fn any_route_echoes_every_method(app: TestApp) {
    let resp = app
        .post("/proxy/npm/lodash")
        .header("x-proxy-key", "secret")
        .body("tarball-bytes")
        .send()
        .await;
    resp.assert_ok();
    assert_eq!(resp.header("x-proxy-method"), Some("POST"));
    assert_eq!(resp.header("x-proxy-path"), Some("/proxy/npm/lodash"));
    assert_eq!(resp.text(), "tarball-bytes");

    let resp = app
        .put("/proxy/cargo/serde")
        .header("x-proxy-key", "secret")
        .body("crate-bytes")
        .send()
        .await;
    resp.assert_ok();
    assert_eq!(resp.header("x-proxy-method"), Some("PUT"));
}

#[r2e::test(app = example_app::ExampleApp)]
async fn fallback_catches_unmatched_routes(app: TestApp) {
    let resp = app.get("/definitely/not/a/route").send().await;
    resp.assert_status(r2e::http::StatusCode::NOT_FOUND);
    let json: serde_json::Value = resp.json();
    assert_eq!(json["error"], "no such route");
    assert_eq!(json["method"], "GET");
    assert_eq!(json["path"], "/definitely/not/a/route");
}
