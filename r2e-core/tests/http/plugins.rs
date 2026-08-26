//! The built-in HTTP plugins wired through `AppBuilder`: panic capture,
//! trailing-slash normalization, the dev-reload endpoints, and CORS.

use r2e_core::builder::AppBuilder;
use r2e_core::builtins::{Cors, DevReload, ErrorHandling, Health, NormalizePath};
use r2e_core::http::{Body, Request, StatusCode};

use crate::support::{raw, raw_get_with, send_get};

// ── ErrorHandling plugin ────────────────────────────────────────────────

#[r2e_core::test]
async fn error_handling_catches_panic() {
    use r2e_core::http::routing::get;

    let app = AppBuilder::new()
        .plugin(ErrorHandling)
        .build_state()
        .await
        .register_routes(r2e_core::http::Router::new().route(
            "/panic",
            get(|| async {
                panic!("boom");
                #[allow(unreachable_code)]
                "never"
            }),
        ))
        .build();

    let (status, body) = send_get(app, "/panic").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["error"], "Internal server error");
}

/// `ErrorHandling` installs a catch-panic layer **at its own install slot**, so
/// instrumentation installed after it (tracing, metrics) sits *outside* and
/// records the panic as a 500 instead of being unwound through.
///
/// This pins the ordering contract: layers apply in install order, and a later
/// install is the outer layer.
#[r2e_core::test]
async fn error_handling_installed_first_lets_later_layers_observe_the_500() {
    use r2e_core::http::routing::get;
    use std::sync::{Arc, Mutex};

    /// Stand-in for the tracing/metrics layer: records the status it observes.
    struct Observer(Arc<Mutex<Vec<u16>>>);

    impl r2e_core::Plugin for Observer {
        type Provided = ();
        type Deps = ();
        type Config = ();
        type Controllers = ();

        async fn build(
            self,
            _deps: (),
            _config: Option<()>,
            ctx: &mut r2e_core::PluginBuildContext,
        ) -> Result<(), r2e_core::PluginBuildError> {
            let seen = Arc::clone(&self.0);
            ctx.add_layer(move |router| {
                router.layer(r2e_core::http::middleware::from_fn(
                    move |req, next: r2e_core::http::middleware::Next| {
                        let seen = Arc::clone(&seen);
                        async move {
                            let resp = next.run(req).await;
                            seen.lock().unwrap().push(resp.status().as_u16());
                            resp
                        }
                    },
                ))
            });
            Ok(())
        }
    }

    let seen = Arc::new(Mutex::new(Vec::new()));
    let app = AppBuilder::new()
        // Install order: catch-panic first (inner), observer second (outer).
        .plugin(ErrorHandling)
        .plugin(Observer(Arc::clone(&seen)))
        .build_state()
        .await
        .register_routes(r2e_core::http::Router::new().route(
            "/panic",
            get(|| async {
                panic!("boom");
                #[allow(unreachable_code)]
                "never"
            }),
        ))
        .build();

    let (status, _) = send_get(app, "/panic").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        *seen.lock().unwrap(),
        vec![500],
        "a layer installed after ErrorHandling must observe the converted 500"
    );
}

// ── NormalizePath plugin ────────────────────────────────────────────────

#[r2e_core::test]
async fn normalize_path_strips_trailing() {
    let router = AppBuilder::new()
        .plugin(Health)
        .plugin(NormalizePath)
        .build_state()
        .await
        .build();
    let (status, body) = send_get(router, "/health/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

// ── DevReload plugin ────────────────────────────────────────────────────

#[r2e_core::test]
async fn dev_reload_status() {
    let router = AppBuilder::new()
        .plugin(DevReload)
        .build_state()
        .await
        .build();
    let (status, body) = send_get(router, "/__r2e_dev/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "dev");
}

#[r2e_core::test]
async fn dev_reload_ping() {
    let router = AppBuilder::new()
        .plugin(DevReload)
        .build_state()
        .await
        .build();
    let (status, body) = send_get(router, "/__r2e_dev/ping").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(json["boot_time"].is_number());
    assert_eq!(json["status"], "ok");
}

// ── E.1 CORS Plugin ──────────────────────────────────────────────────────

#[r2e_core::test]
async fn cors_permissive_allows_origin() {
    let router = AppBuilder::new()
        .plugin(Health)
        .plugin(Cors::permissive())
        .build_state()
        .await
        .build();
    let resp = raw_get_with(router, "/health", &[("origin", "http://example.com")]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers().get("access-control-allow-origin").is_some(),
        "response should have access-control-allow-origin header"
    );
}

#[r2e_core::test]
async fn cors_preflight_returns_200() {
    let router = AppBuilder::new()
        .plugin(Health)
        .plugin(Cors::permissive())
        .build_state()
        .await
        .build();
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
    let router = AppBuilder::new()
        .plugin(Health)
        .plugin(NormalizePath)
        .build_state()
        .await
        .build();
    // /health/ with query string should redirect to /health?foo=bar
    let (status, body) = send_get(router, "/health/?foo=bar").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

#[r2e_core::test]
async fn normalize_path_root_slash_unaffected() {
    // GET / with no root route should return 404, not a redirect loop
    let router = AppBuilder::new()
        .plugin(Health)
        .plugin(NormalizePath)
        .build_state()
        .await
        .build();
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

    let router = AppBuilder::new()
        .plugin(NormalizePath)
        .build_state()
        .await
        .register_routes(
            r2e_core::http::Router::new().route("/users/{id}", get(|| async { "user" })),
        )
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
    let router = AppBuilder::new()
        .plugin(Health)
        .plugin(NormalizePath)
        .build_state()
        .await
        .build();
    let (status, body) = send_get(router, "http://test//health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}
