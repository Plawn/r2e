use crate::{openapi_routes, OpenApiConfig};
use quarlus_core::Plugin;

/// Plugin that adds OpenAPI spec generation and optional documentation UI.
///
/// # Example
///
/// ```ignore
/// use quarlus_openapi::{OpenApiPlugin, OpenApiConfig};
///
/// AppBuilder::new()
///     .build_state::<Services>()
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
    fn install<T: Clone + Send + Sync + 'static>(self, app: quarlus_core::AppBuilder<T>) -> quarlus_core::AppBuilder<T> {
        let config = self.config;
        app.with_openapi_builder(move |metadata| openapi_routes::<T>(config, metadata))
    }
}
