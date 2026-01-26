use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use quarlus_core::openapi::RouteInfo;
use std::sync::Arc;

use crate::builder::{build_spec, OpenApiConfig};

struct OpenApiState {
    spec_json: String,
}

/// Build an `axum::Router` that serves `/openapi.json` and optionally `/swagger-ui`.
///
/// The returned router can be passed to `AppBuilder::register_routes()`.
pub fn openapi_routes<T: Clone + Send + Sync + 'static>(
    config: OpenApiConfig,
    route_metadata: Vec<Vec<RouteInfo>>,
) -> Router<T> {
    let all_routes: Vec<RouteInfo> = route_metadata.into_iter().flatten().collect();
    let spec = build_spec(&config, &all_routes);
    let spec_json = serde_json::to_string_pretty(&spec).unwrap_or_else(|_| "{}".to_string());
    let swagger_ui = config.swagger_ui;

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

    if swagger_ui {
        let state_for_ui = state.clone();
        router = router.route(
            "/swagger-ui",
            get(move || {
                let _ = &state_for_ui;
                async move {
                    Html(SWAGGER_UI_HTML).into_response()
                }
            }),
        );
    }

    router
}

const SWAGGER_UI_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <title>Swagger UI</title>
    <link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css">
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
    <script>
        SwaggerUIBundle({
            url: '/openapi.json',
            dom_id: '#swagger-ui',
        });
    </script>
</body>
</html>"#;
