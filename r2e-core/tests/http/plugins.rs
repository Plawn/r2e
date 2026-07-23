//! The built-in HTTP plugins wired through `AppBuilder`: panic capture,
//! trailing-slash normalization, the dev-reload endpoints, and CORS.

use r2e_core::builder::AppBuilder;
use r2e_core::http::{Body, Request, StatusCode};
use r2e_core::plugins::{Cors, DevReload, ErrorHandling, Health, NormalizePath};

use crate::support::{raw, raw_get_with, send_get};

fn build_app() -> AppBuilder<()> {
    AppBuilder::new().with_state(())
}

// ── ErrorHandling plugin ────────────────────────────────────────────────

#[r2e_core::test]
async fn error_handling_catches_panic() {
    use r2e_core::http::routing::get;

    let app = AppBuilder::new()
        .with_state(())
        .register_routes(r2e_core::http::Router::new().route(
            "/panic",
            get(|| async {
                panic!("boom");
                #[allow(unreachable_code)]
                "never"
            }),
        ))
        .with(ErrorHandling)
        .build();

    let (status, body) = send_get(app, "/panic").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["error"], "Internal server error");
}

// ── NormalizePath plugin ────────────────────────────────────────────────

#[r2e_core::test]
async fn normalize_path_strips_trailing() {
    let router = build_app().with(Health).with(NormalizePath).build();
    let (status, body) = send_get(router, "/health/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

// ── DevReload plugin ────────────────────────────────────────────────────

#[r2e_core::test]
async fn dev_reload_status() {
    let router = build_app().with(DevReload).build();
    let (status, body) = send_get(router, "/__r2e_dev/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "dev");
}

#[r2e_core::test]
async fn dev_reload_ping() {
    let router = build_app().with(DevReload).build();
    let (status, body) = send_get(router, "/__r2e_dev/ping").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(json["boot_time"].is_number());
    assert_eq!(json["status"], "ok");
}

// ── E.1 CORS Plugin ──────────────────────────────────────────────────────

#[r2e_core::test]
async fn cors_permissive_allows_origin() {
    let router = build_app().with(Health).with(Cors::permissive()).build();
    let resp = raw_get_with(router, "/health", &[("origin", "http://example.com")]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers().get("access-control-allow-origin").is_some(),
        "response should have access-control-allow-origin header"
    );
}

#[r2e_core::test]
async fn cors_preflight_returns_200() {
    let router = build_app().with(Health).with(Cors::permissive()).build();
    let resp = raw(
        router,
        "OPTIONS",
        "/health",
        &[
            ("origin", "http://example.com"),
            ("access-control-request-method", "GET"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get("access-control-allow-origin").is_some());
    assert!(resp.headers().get("access-control-allow-methods").is_some());
}

// ── E.5 NormalizePath edge cases ──────────────────────────────────────────

#[r2e_core::test]
async fn normalize_path_preserves_query_string() {
    let router = build_app().with(Health).with(NormalizePath).build();
    // /health/ with query string should redirect to /health?foo=bar
    let (status, body) = send_get(router, "/health/?foo=bar").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

#[r2e_core::test]
async fn normalize_path_root_slash_unaffected() {
    // GET / with no root route should return 404, not a redirect loop
    let router = build_app().with(Health).with(NormalizePath).build();
    let (status, _) = send_get(router, "/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[r2e_core::test]
async fn normalize_path_preserves_matched_path_for_outer_layers() {
    // The rewrite happens BEFORE routing, so a trailing-slash request is
    // routed exactly once and instrumentation layers (Prometheus, OTel)
    // added via `with_layer_fn` see the `MatchedPath` route template —
    // not the "unmatched" sentinel a fallback re-dispatch would leave.
    use r2e_core::http::extract::MatchedPath;
    use r2e_core::http::middleware::{from_fn, Next};
    use r2e_core::http::routing::get;

    let router = build_app()
        .register_routes(
            r2e_core::http::Router::new().route("/users/{id}", get(|| async { "user" })),
        )
        .with(NormalizePath)
        .with_layer_fn(|router| {
            router.layer(from_fn(|req: Request<Body>, next: Next| async move {
                let label = req
                    .extensions()
                    .get::<MatchedPath>()
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_else(|| "unmatched".to_string());
                let mut resp = next.run(req).await;
                resp.headers_mut()
                    .insert("x-matched-path", label.parse().unwrap());
                resp
            }))
        })
        .build();

    let resp = raw_get_with(router, "/users/42/", &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["x-matched-path"], "/users/{id}");
}

#[r2e_core::test]
async fn normalize_path_collapses_leading_slashes() {
    // tower-http's trim_trailing_slash also collapses a leading run of
    // slashes (`//health` → `/health`) — documented plugin behavior.
    // Absolute-form URI keeps `//health` as the path (origin-form `//x`
    // would parse as an authority).
    let router = build_app().with(Health).with(NormalizePath).build();
    let (status, body) = send_get(router, "http://test//health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}
