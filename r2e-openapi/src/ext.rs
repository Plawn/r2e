use crate::{openapi_routes, OpenApiConfig};
use r2e_core::plugin::{Plugin, PluginBuildContext, PluginBuildError};
use r2e_core::ErrorProjector;

/// Plugin that adds OpenAPI spec generation and optional documentation UI.
///
/// It mounts `/openapi.json` (and, with `with_docs_ui(true)`, `/docs`) from a
/// **Routes-stage** effect: the spec is built once every controller has
/// registered, so install order does not matter — no more "install this one
/// last".
///
/// # Example
///
/// ```ignore
/// use r2e_openapi::{OpenApiPlugin, OpenApiConfig};
///
/// AppBuilder::new()
///     .plugin(OpenApiPlugin::new(
///         OpenApiConfig::new("My API", "1.0.0")
///             .with_docs_ui(true),
///     ))
///     .build_state()
///     .await
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
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        let mut config = self.config;
        ctx.after_routes(move |routes| {
            // The application's envelope (`AppBuilder::error_projection::<E>()`)
            // documents every route without an envelope of its own. An explicit
            // `with_error_schema` on the config wins.
            if config.error_schema.is_none() {
                if let Some(projector) = routes.bean_context().try_get::<ErrorProjector>() {
                    config.error_schema = Some(*projector.schema());
                }
            }
            let router = openapi_routes::<()>(config, routes.routes());
            routes.register_routes(router);
        });
        Ok(())
    }
}
