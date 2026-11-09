//! `OpenApiPlugin` on the typed-config contract: `openapi.*` drives
//! `OpenApiPlugin::default()`, `new(cfg)` wins over the file, and
//! `openapi.enabled: false` removes the routes.

use http_body_util::BodyExt;
use r2e_core::config::R2eConfig;
use r2e_core::http::{Body, Request, Routes, StatusCode};
use r2e_core::AppBuilder;
use r2e_openapi::{OpenApiConfig, OpenApiPlugin, OpenApiPluginConfig};
use serde_json::Value;
use tower::ServiceExt;

async fn app(plugin: OpenApiPlugin, yaml: &str) -> Routes {
    AppBuilder::new()
        .override_config(R2eConfig::from_yaml_str(yaml).expect("valid yaml"))
        .load_config::<()>()
        .plugin(plugin)
        .build_state()
        .await
        .build()
}

async fn get(router: &Routes, path: &str) -> (StatusCode, Vec<u8>) {
    let resp = router
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, bytes.to_vec())
}

async fn info(router: &Routes) -> Value {
    let (status, body) = get(router, "/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    let spec: Value = serde_json::from_slice(&body).unwrap();
    spec["info"].clone()
}

const SECTION: &str = "openapi:\n  title: File API\n  version: 2.3.4\n  description: From the file\n  docs-ui: true\n";

#[r2e_core::test]
async fn default_plugin_is_file_driven() {
    let router = app(OpenApiPlugin::default(), SECTION).await;
    let info = info(&router).await;
    assert_eq!(info["title"], "File API");
    assert_eq!(info["version"], "2.3.4");
    assert_eq!(info["description"], "From the file");
    let (status, _) = get(&router, "/docs").await;
    assert_eq!(status, StatusCode::OK, "docs-ui: true mounts /docs");
}

#[r2e_core::test]
async fn default_plugin_without_section_uses_defaults_and_no_docs_ui() {
    let router = app(OpenApiPlugin::default(), "server:\n  port: 0\n").await;
    let info = info(&router).await;
    assert_eq!(info["title"], "API");
    assert_eq!(info["version"], "0.0.0");
    assert!(info.get("description").is_none_or(Value::is_null));
    let (status, _) = get(&router, "/docs").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[r2e_core::test]
async fn explicit_config_wins_but_description_falls_through() {
    let router = app(OpenApiPlugin::new(OpenApiConfig::new("Code API", "9.0.0")), SECTION).await;
    let info = info(&router).await;
    assert_eq!(info["title"], "Code API");
    assert_eq!(info["version"], "9.0.0");
    assert_eq!(info["description"], "From the file");
    let (status, _) = get(&router, "/docs").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "explicit docs_ui=false wins over the file");
}

#[r2e_core::test]
async fn disabled_by_config_removes_both_routes() {
    let router = app(
        OpenApiPlugin::new(OpenApiConfig::new("t", "1").with_docs_ui(true)),
        "openapi:\n  enabled: false\n",
    )
    .await;
    assert_eq!(get(&router, "/openapi.json").await.0, StatusCode::NOT_FOUND);
    assert_eq!(get(&router, "/docs").await.0, StatusCode::NOT_FOUND);
}

#[test]
fn resolve_merges_builder_over_file() {
    let file = Some(OpenApiPluginConfig {
        title: Some("f".into()),
        version: Some("1".into()),
        description: Some("d".into()),
        docs_ui: Some(true),
    });
    let r = OpenApiPlugin::resolve(None, file.clone());
    assert_eq!((r.title.as_str(), r.version.as_str(), r.docs_ui), ("f", "1", true));
    assert_eq!(r.description.as_deref(), Some("d"));

    let r = OpenApiPlugin::resolve(Some(OpenApiConfig::new("c", "2").with_description("own")), file);
    assert_eq!((r.title.as_str(), r.version.as_str(), r.docs_ui), ("c", "2", false));
    assert_eq!(r.description.as_deref(), Some("own"));

    let r = OpenApiPlugin::resolve(None, None);
    assert_eq!((r.title.as_str(), r.version.as_str()), ("API", "0.0.0"));
}
