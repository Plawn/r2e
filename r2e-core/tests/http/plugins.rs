//! The built-in HTTP plugins wired through `AppBuilder`: trailing-slash
//! normalization, the dev-reload endpoints, and CORS. Panic capture is not a
//! plugin — see `panic.rs`.

use std::time::Duration;

use r2e_core::builder::AppBuilder;
use r2e_core::builtins::cors::CorsSettings;
use r2e_core::builtins::{Cors, CorsConfig, DevReload, Health, NormalizePath};
use r2e_core::config::R2eConfig;
use r2e_core::http::{Body, Request, Response, Routes, StatusCode};

use crate::support::{raw, raw_get_with, send_get};

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
    use r2e_core::http::middleware::Next;
    use r2e_core::http::routing::get;

    let router = AppBuilder::new()
        .plugin(NormalizePath)
        .build_state()
        .await
        .register_routes(
            r2e_core::http::Routes::new().route("/users/{id}", get(|_, _| async { "user" })),
        )
        .with_layer_fn(|router| {
            router.middleware_fn(|req: Request<Body>, next: Next| async move {
                let label = req
                    .extensions()
                    .get::<MatchedPath>()
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_else(|| "unmatched".to_string());
                let mut resp = next.run(req).await;
                resp.headers_mut()
                    .insert("x-matched-path", label.parse().unwrap());
                resp
            })
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

// ── E.2 CORS Plugin — `cors.*` typed config ─────────────────────────────────
//
// Builder setter > file (`cors.*`) > `default_origin(s)`; fail-closed when
// nothing sets origins (except under `dev`), every origin validated at boot.

async fn cors_app(plugin: Cors, yaml: &str) -> Routes {
    AppBuilder::new()
        .override_config(R2eConfig::from_yaml_str(yaml).expect("valid yaml"))
        .load_config::<()>()
        .plugin(Health)
        .plugin(plugin)
        .build_state()
        .await
        .build()
}

async fn cors_boot_error(plugin: Cors, yaml: &str, profile: Option<&str>) -> String {
    let mut builder = AppBuilder::new()
        .override_config(R2eConfig::from_yaml_str(yaml).expect("valid yaml"));
    if let Some(profile) = profile {
        builder = builder.with_profile(profile);
    }
    let err = builder
        .load_config::<()>()
        .plugin(Health)
        .plugin(plugin)
        .try_build_state()
        .await
        .err()
        .expect("boot must fail");
    err.to_string()
}

async fn preflight(router: Routes, origin: &str) -> Response {
    raw(
        router,
        "OPTIONS",
        "/health",
        &[("origin", origin), ("access-control-request-method", "GET")],
        Body::empty(),
    )
    .await
}

fn header<'a>(resp: &'a Response, name: &str) -> Option<&'a str> {
    resp.headers().get(name).map(|v| v.to_str().unwrap())
}

const NO_CORS_SECTION: &str = "server:\n  port: 0\n";

#[r2e_core::test]
async fn cors_default_origin_applies_when_section_absent() {
    let router = cors_app(
        Cors::new().default_origin("http://localhost:3000"),
        NO_CORS_SECTION,
    )
    .await;
    let resp = preflight(router.clone(), "http://localhost:3000").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        header(&resp, "access-control-allow-origin"),
        Some("http://localhost:3000")
    );
    let resp = preflight(router, "http://evil.example").await;
    assert_eq!(
        header(&resp, "access-control-allow-origin"),
        None,
        "an unlisted origin is not mirrored"
    );
}

#[r2e_core::test]
async fn cors_file_origins_win_over_default_origin() {
    let router = cors_app(
        Cors::new().default_origin("http://localhost:3000"),
        "cors:\n  allowed-origins:\n    - https://app.example.com\n    - http://localhost:*\n",
    )
    .await;
    for (origin, allowed) in [
        ("https://app.example.com", true),
        ("http://localhost:5173", true),
        ("http://localhost:3000", true),
        ("https://localhost:5173", false),
        ("https://app.example.com.evil.io", false),
    ] {
        let resp = preflight(router.clone(), origin).await;
        assert_eq!(
            header(&resp, "access-control-allow-origin"),
            allowed.then_some(origin),
            "{origin}"
        );
    }
}

#[r2e_core::test]
async fn cors_star_in_file_is_permissive() {
    let router = cors_app(Cors::new(), "cors:\n  allowed-origins: [\"*\"]\n").await;
    let resp = preflight(router, "http://anything.example").await;
    assert_eq!(header(&resp, "access-control-allow-origin"), Some("*"));
    assert_eq!(header(&resp, "access-control-allow-methods"), Some("*"));
    assert_eq!(header(&resp, "access-control-allow-headers"), Some("*"));
}

#[r2e_core::test]
async fn cors_file_methods_headers_credentials_max_age() {
    let router = cors_app(
        Cors::new(),
        "cors:\n  allowed-origins: [https://app.example.com]\n  allowed-methods: [get, POST]\n  allowed-headers: [authorization, content-type]\n  expose-headers: [x-request-id]\n  allow-credentials: true\n  max-age: 2h\n",
    )
    .await;
    let resp = preflight(router, "https://app.example.com").await;
    assert_eq!(
        header(&resp, "access-control-allow-origin"),
        Some("https://app.example.com")
    );
    assert_eq!(header(&resp, "access-control-allow-methods"), Some("GET,POST"));
    assert_eq!(
        header(&resp, "access-control-allow-headers"),
        Some("authorization,content-type")
    );
    assert_eq!(header(&resp, "access-control-allow-credentials"), Some("true"));
    assert_eq!(header(&resp, "access-control-max-age"), Some("7200"));
}

#[r2e_core::test]
async fn cors_permissive_wins_over_file_section() {
    let router = cors_app(
        Cors::permissive(),
        "cors:\n  allowed-origins: [https://app.example.com]\n",
    )
    .await;
    let resp = preflight(router, "http://other.example").await;
    assert_eq!(header(&resp, "access-control-allow-origin"), Some("*"));
}

#[r2e_core::test]
async fn cors_builder_setter_wins_over_file() {
    let router = cors_app(
        Cors::new().allowed_origins(["http://builder.example"]),
        "cors:\n  allowed-origins: [https://file.example]\n",
    )
    .await;
    let resp = preflight(router.clone(), "http://builder.example").await;
    assert_eq!(
        header(&resp, "access-control-allow-origin"),
        Some("http://builder.example")
    );
    let resp = preflight(router, "https://file.example").await;
    assert_eq!(header(&resp, "access-control-allow-origin"), None);
}

#[r2e_core::test]
async fn cors_disabled_by_config_installs_nothing() {
    let router = cors_app(
        Cors::permissive(),
        "cors:\n  enabled: false\n  allowed-origins: [\"*\"]\n",
    )
    .await;
    let resp = raw_get_with(router, "/health", &[("origin", "http://example.com")]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header(&resp, "access-control-allow-origin"), None);
}

#[r2e_core::test]
async fn cors_invalid_origin_fails_the_boot() {
    let err = cors_boot_error(
        Cors::new(),
        "cors:\n  allowed-origins: [\"http://app.example.com/\"]\n",
        None,
    )
    .await;
    assert!(err.contains("cors.allowed-origins"), "{err}");
    assert!(err.contains("\"http://app.example.com/\""), "{err}");
}

#[r2e_core::test]
async fn cors_no_origins_outside_dev_fails_the_boot() {
    let err = cors_boot_error(Cors::new(), NO_CORS_SECTION, None).await;
    assert!(err.contains("cors.allowed-origins"), "{err}");
    assert!(err.contains("default_origin"), "{err}");
    let err = cors_boot_error(Cors::new(), NO_CORS_SECTION, Some("prod")).await;
    assert!(err.contains("cors.allowed-origins"), "{err}");
}

#[r2e_core::test]
async fn cors_no_origins_under_dev_is_permissive() {
    let router = AppBuilder::new()
        .with_profile("dev")
        .load_config::<()>()
        .plugin(Health)
        .plugin(Cors::new())
        .build_state()
        .await
        .build();
    let resp = preflight(router, "http://anything.example").await;
    assert_eq!(header(&resp, "access-control-allow-origin"), Some("*"));
}

#[r2e_core::test]
async fn cors_credentials_with_wildcards_fail_the_boot() {
    let err = cors_boot_error(
        Cors::new(),
        "cors:\n  allowed-origins: [\"*\"]\n  allow-credentials: true\n",
        None,
    )
    .await;
    assert!(err.contains("allow-credentials"), "{err}");
    let err = cors_boot_error(
        Cors::new(),
        "cors:\n  allowed-origins: [https://app.example.com]\n  allow-credentials: true\n",
        None,
    )
    .await;
    assert!(err.contains("allowed-methods"), "{err}");
}

#[r2e_core::test]
async fn cors_invalid_method_or_header_fails_the_boot() {
    let err = cors_boot_error(
        Cors::new(),
        "cors:\n  allowed-origins: [\"*\"]\n  allowed-methods: [\"GE T\"]\n",
        None,
    )
    .await;
    assert!(err.contains("allowed-methods"), "{err}");
    let err = cors_boot_error(
        Cors::new(),
        "cors:\n  allowed-origins: [\"*\"]\n  allowed-headers: [\"bad header\"]\n",
        None,
    )
    .await;
    assert!(err.contains("allowed-headers"), "{err}");
}

// Pure resolver: the precedence tiers without an app.

fn file(origins: &[&str]) -> Option<CorsConfig> {
    Some(CorsConfig {
        allowed_origins: Some(origins.iter().map(|s| s.to_string()).collect()),
        ..CorsConfig::default()
    })
}

fn origins(settings: &CorsSettings) -> Vec<&str> {
    settings.origins.iter().map(String::as_str).collect()
}

#[test]
fn resolver_precedence_builder_file_default() {
    let plugin = Cors::new()
        .allowed_origins(["http://builder"])
        .default_origin("http://default");
    let s = plugin.resolve_settings(file(&["http://file"]), "prod").unwrap();
    assert_eq!(origins(&s), ["http://builder"]);

    let plugin = Cors::new().default_origin("http://default");
    let s = plugin.resolve_settings(file(&["http://file"]), "prod").unwrap();
    assert_eq!(origins(&s), ["http://file"]);
    let s = plugin.resolve_settings(None, "prod").unwrap();
    assert_eq!(origins(&s), ["http://default"]);

    let s = Cors::new().resolve_settings(None, "dev").unwrap();
    assert_eq!(origins(&s), ["*"]);
    assert!(Cors::new().resolve_settings(None, "prod").is_err());
    assert!(Cors::new().resolve_settings(file(&[]), "dev").is_err(), "an empty list is an error even under dev");
}

#[test]
fn resolver_builder_fields_override_file_fields_individually() {
    let plugin = Cors::new().max_age(Duration::from_secs(10));
    let cfg = CorsConfig {
        allowed_origins: Some(vec!["https://a".into()]),
        allowed_methods: Some(vec!["get".into()]),
        max_age: Some(Duration::from_secs(99)),
        ..CorsConfig::default()
    };
    let s = plugin.resolve_settings(Some(cfg), "prod").unwrap();
    assert_eq!(s.max_age, Some(Duration::from_secs(10)));
    assert_eq!(s.methods.as_deref().map(|m| m.len()), Some(1));
    assert!(s.headers.is_none());
    assert!(!s.allow_credentials);
}
