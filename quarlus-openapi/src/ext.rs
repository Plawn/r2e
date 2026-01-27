use crate::{openapi_routes, OpenApiConfig};
use quarlus_core::AppBuilder;

/// Extension trait to add `.with_openapi()` to `AppBuilder`.
pub trait AppBuilderOpenApiExt<T: Clone + Send + Sync + 'static> {
    fn with_openapi(self, config: OpenApiConfig) -> Self;
}

impl<T: Clone + Send + Sync + 'static> AppBuilderOpenApiExt<T> for AppBuilder<T> {
    fn with_openapi(self, config: OpenApiConfig) -> Self {
        self.with_openapi_builder(move |metadata| openapi_routes::<T>(config, metadata))
    }
}
