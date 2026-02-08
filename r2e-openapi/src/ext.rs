use crate::{openapi_routes, OpenApiConfig};
use r2e_core::meta::RouteInfo;
use r2e_core::Plugin;

/// Plugin that adds OpenAPI spec generation and optional documentation UI.
///
/// # Example
///
/// ```ignore
/// use r2e_openapi::{OpenApiPlugin, OpenApiConfig};
///
/// AppBuilder::new()
///     .build_state::<Services>()
///     .await
///     .with(OpenApiPlugin::new(
///         OpenApiConfig::new("My API", "1.0.0")
///             .with_docs_ui(true),
///     ))
/// ```
pub struct OpenApiPlugin {
    config: OpenApiConfig,
}

impl OpenApiPlugin {
    /// Create a new OpenAPI plugin with the given configuration.
    pub fn new(config: OpenApiConfig) -> Self {
        Self { config }
    }
}

impl Plugin for OpenApiPlugin {
    fn install<T: Clone + Send + Sync + 'static>(self, app: r2e_core::AppBuilder<T>) -> r2e_core::AppBuilder<T> {
        let config = self.config;
        app.with_meta_consumer::<RouteInfo, _>(move |routes| openapi_routes::<T>(config, routes))
    }
}
