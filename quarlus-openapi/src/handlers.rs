use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use quarlus_core::openapi::RouteInfo;
use std::sync::Arc;

use crate::builder::{build_spec, OpenApiConfig};

const WTI_CSS: &str = include_str!("../assets/wti-element.css");
const WTI_JS: &str = include_str!("../assets/wti-element.iife.js");

struct OpenApiState {
    spec_json: String,
}

/// Build an `axum::Router` that serves `/openapi.json` and optionally `/docs`.
///
/// The returned router can be passed to `AppBuilder::register_routes()`.
pub fn openapi_routes<T: Clone + Send + Sync + 'static>(
    config: OpenApiConfig,
    route_metadata: Vec<Vec<RouteInfo>>,
) -> Router<T> {
    let all_routes: Vec<RouteInfo> = route_metadata.into_iter().flatten().collect();
    let spec = build_spec(&config, &all_routes);
    let spec_json = serde_json::to_string_pretty(&spec).unwrap_or_else(|_| "{}".to_string());
    let docs_ui = config.docs_ui;

    let state = Arc::new(OpenApiState {
        spec_json,
    });

    let state_clone = state.clone();
    let mut router = Router::<T>::new().route(
        "/openapi.json",
        get(move || {
            let json = state_clone.spec_json.clone();
            async move {
                (
                    [("content-type", "application/json")],
                    json,
                )
                    .into_response()
            }
        }),
    );

    if docs_ui {
        let state_for_ui = state.clone();
        router = router
            .route(
                "/docs",
                get(move || {
                    let _ = &state_for_ui;
                    async move {
                        Html(WTI_HTML).into_response()
                    }
                }),
            )
            .route(
                "/docs/wti-element.css",
                get(|| async {
                    (
                        [("content-type", "text/css")],
                        WTI_CSS,
                    )
                        .into_response()
                }),
            )
            .route(
                "/docs/wti-element.js",
                get(|| async {
                    (
                        [("content-type", "application/javascript")],
                        WTI_JS,
                    )
                        .into_response()
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
