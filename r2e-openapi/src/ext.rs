use crate::{openapi_routes, OpenApiConfig};
use r2e_core::plugin::{Plugin, PluginBuildContext, PluginBuildError};
use r2e_core::prelude::ConfigProperties;
use r2e_core::ErrorProjector;

/// Typed view of the `openapi.*` section (every key optional;
/// `openapi.enabled` is read by the plugin contract itself).
///
/// ```yaml
/// openapi:
///   title: My API
///   version: 1.2.0
///   description: What this service does
///   docs-ui: true        # mount Swagger UI at /docs
///   enabled: true        # false hides /openapi.json and /docs entirely
/// ```
#[derive(ConfigProperties, Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenApiPluginConfig {
    /// `openapi.title` — `info.title` (default `"API"`).
    pub title: Option<String>,
    /// `openapi.version` — `info.version` (default `"0.0.0"`).
    pub version: Option<String>,
    /// `openapi.description` — `info.description`.
    pub description: Option<String>,
    /// `openapi.docs-ui` — serve Swagger UI at `/docs` (default `false`).
    #[config(key = "docs-ui")]
    pub docs_ui: Option<bool>,
}

/// Plugin that adds OpenAPI spec generation and optional documentation UI.
///
/// It mounts `/openapi.json` (and, with `docs-ui`, `/docs`) from a
/// **Routes-stage** effect: the spec is built once every controller has
/// registered, so install order does not matter — no more "install this one
/// last".
///
/// Two ways in, like every configurable plugin (builder > file > default):
///
/// - `OpenApiPlugin::default()` — file-driven: title, version, description
///   and `docs-ui` come from the `openapi.*` section
///   ([`OpenApiPluginConfig`]).
/// - `OpenApiPlugin::new(cfg)` — programmatic [`OpenApiConfig`] (needed for
///   schema registries, overrides and a custom error schema). Its
///   `title`/`version`/`docs_ui` win over the file; a `None` description
///   falls through to `openapi.description`.
///
/// `openapi.enabled: false` removes both routes — the usual way to keep the
/// spec off a production deployment without touching the code.
///
/// # Example
///
/// ```ignore
/// use r2e_openapi::OpenApiPlugin;
///
/// AppBuilder::new()
///     .load_config::<AppConfig>()
///     .plugin(OpenApiPlugin::default())   // reads `openapi.*`
///     .build_state()
///     .await
/// ```
pub struct OpenApiPlugin {
    config: Option<OpenApiConfig>,
}

impl Default for OpenApiPlugin {
    /// File-driven: everything comes from the `openapi.*` section.
    fn default() -> Self {
        Self { config: None }
    }
}

impl OpenApiPlugin {
    /// Create a new OpenAPI plugin with the given configuration (its explicit
    /// values win over the `openapi.*` section).
    pub fn new(config: OpenApiConfig) -> Self {
        Self {
            config: Some(config),
        }
    }

    /// Merge the builder-side config with the `openapi.*` section:
    /// builder > file > default. Exposed for tests.
    #[doc(hidden)]
    pub fn resolve(
        config: Option<OpenApiConfig>,
        file: Option<OpenApiPluginConfig>,
    ) -> OpenApiConfig {
        let file = file.unwrap_or_default();
        match config {
            Some(mut config) => {
                if config.description.is_none() {
                    config.description = file.description;
                }
                config
            }
            None => {
                let mut config = OpenApiConfig::new(
                    file.title.as_deref().unwrap_or("API"),
                    file.version.as_deref().unwrap_or("0.0.0"),
                )
                .with_docs_ui(file.docs_ui.unwrap_or(false));
                config.description = file.description;
                config
            }
        }
    }
}

impl Plugin for OpenApiPlugin {
    type Provided = ();
    type Deps = ();
    type Config = OpenApiPluginConfig;
    type Controllers = ();

    const CONFIG_PREFIX: Option<&'static str> = Some("openapi");

    async fn build(
        self,
        _deps: Self::Deps,
        config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        if !ctx.enabled() {
            tracing::debug!(target: "r2e::openapi", "openapi: disabled by `openapi.enabled = false`");
            return Ok(());
        }
        let mut config = Self::resolve(self.config, config);
        ctx.after_routes(move |routes| {
            // The application's envelope (`AppBuilder::error_projection::<E>()`)
            // documents every route without an envelope of its own. An explicit
            // `with_error_schema` on the config wins.
            if config.error_schema.is_none()
                && let Some(projector) = routes.bean_context().try_get::<ErrorProjector>() {
                    config.error_schema = Some(*projector.schema());
                }
            let router = openapi_routes::<()>(config, routes.routes());
            routes.register_routes(router);
        });
        Ok(())
    }
}
