use r2e_core::http::response::{Html, IntoResponse};
use r2e_core::http::routing::get;
use r2e_core::http::{Bytes, Router};
use r2e_core::di::meta::RouteInfo;
use std::sync::Arc;

use crate::builder::{build_spec, OpenApiConfig};

const WTI_CSS: &str = include_str!("../assets/wti-element.css");
const WTI_JS: &str = include_str!("../assets/wti-element.iife.js");

/// The rendered spec, pre-encoded once at router build time.
///
/// `Bytes` and not `String`: the `/openapi.json` handler needs an owned body
/// per request, and the spec is immutable for the app's lifetime — cloning a
/// `String` would re-allocate and memcpy the whole document (tens to hundreds
/// of kB on a real app) on every hit, while cloning `Bytes` bumps a refcount.
struct OpenApiState {
    spec_json: Bytes,
}

/// Build an `r2e::http::Router` that serves `/openapi.json` and optionally `/docs`.
///
/// The returned router can be passed to `AppBuilder::register_routes()`.
pub fn openapi_routes<T: Clone + Send + Sync + 'static>(
    config: OpenApiConfig,
    routes: &[RouteInfo],
) -> Router<T> {
    let spec = build_spec(&config, &routes);
    let spec_json = Bytes::from(
        serde_json::to_vec_pretty(&spec)
            .expect("OpenAPI spec is a serde_json::Value and serializes infallibly"),
    );
    let docs_ui = config.docs_ui;

    let state = Arc::new(OpenApiState { spec_json });

    let state_clone = state.clone();
    let mut router = Router::<T>::new().route(
        "/openapi.json",
        get(move || {
            let json = state_clone.spec_json.clone();
            async move { ([("content-type", "application/json")], json).into_response() }
        }),
    );

    if docs_ui {
        let state_for_ui = state.clone();
        router = router
            .route(
                "/docs",
                get(move || {
                    let _ = &state_for_ui;
                    async move { Html(WTI_HTML).into_response() }
                }),
            )
            .route(
                "/docs/wti-element.css",
                get(|| async { ([("content-type", "text/css")], WTI_CSS).into_response() }),
            )
            .route(
                "/docs/wti-element.js",
                get(|| async {
                    ([("content-type", "application/javascript")], WTI_JS).into_response()
                }),
            );
    }

    router
}

const WTI_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>API Documentation</title>
    <link rel="stylesheet" href="/docs/wti-element.css">
</head>
<body>
    <wti-element
        spec-url="/openapi.json"
        theme="dark"
        locale="en"
    ></wti-element>
    <script src="/docs/wti-element.js"></script>
</body>
</html>"#;
