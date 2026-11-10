//! The `Cors` plugin: CORS headers driven by the `cors.*` section.
//!
//! Like every configurable plugin, it declares `type Config` and merges the
//! tiers through [`Layered`]: **builder setter > file (`cors.*`) > preset
//! ([`Cors::preset`], `default_origin`) > default**. The section is
//! loaded inside `build_state()`, so `.plugin(Cors::new())` goes anywhere in
//! the chain, before or after `load_config()` — never read
//! `AppBuilder::r2e_config()` mid-chain to construct it.
//!
//! ```yaml
//! cors:
//!   allowed-origins:            # "*" | exact origin | scheme://host:*
//!     - https://app.example.com
//!     - http://localhost:*
//!   allowed-methods: [GET, POST, PUT, DELETE]   # default: any
//!   allowed-headers: [authorization, content-type]  # default: any
//!   expose-headers: [x-request-id]
//!   allow-credentials: true
//!   max-age: 2h
//! ```
//!
//! Origins are a YAML **list**; `${VAR}` expands to one entry, not a
//! comma-separated list. Entries are validated at boot — a typo such as
//! `http://app.example.com/` fails `build_state()` naming the entry — with
//! the grammar shared with `mcp.allowed-origins`
//! ([`validate_origin`](crate::http::layers::validate_origin)).

use std::time::Duration;

use crate::config::Layered;
use crate::http::header::HeaderName;
use crate::http::layers::cors::{self, CorsLayer};
use crate::http::layers::{allow_origins, validate_origin};
use crate::http::Method;
use crate::plugin::{Plugin, PluginBuildContext, PluginBuildError};

/// Typed view of the `cors.*` section (every key optional; `cors.enabled` is
/// read by the plugin contract itself).
#[derive(r2e_macros::ConfigProperties, Clone, Debug, Default, PartialEq, Eq)]
pub struct CorsConfig {
    /// `cors.allowed-origins` — `*`, exact origins, or `scheme://host:*`.
    #[config(key = "allowed-origins")]
    pub allowed_origins: Option<Vec<String>>,
    /// `cors.allowed-methods` — HTTP method names; unset = any method.
    #[config(key = "allowed-methods")]
    pub allowed_methods: Option<Vec<String>>,
    /// `cors.allowed-headers` — request header names; unset = any header.
    #[config(key = "allowed-headers")]
    pub allowed_headers: Option<Vec<String>>,
    /// `cors.expose-headers` — response headers readable by the browser.
    #[config(key = "expose-headers")]
    pub expose_headers: Option<Vec<String>>,
    /// `cors.allow-credentials` — incompatible with `*` origins / any
    /// methods / any headers (checked at boot).
    #[config(key = "allow-credentials")]
    pub allow_credentials: Option<bool>,
    /// `cors.max-age` — preflight cache lifetime (`7200`, `"2h"`).
    #[config(key = "max-age")]
    pub max_age: Option<Duration>,
}

/// CORS plugin.
///
/// - [`Cors::new()`] (or `Cors::default()`): configured by the `cors.*`
///   section; the builder setters below override the file, and
///   [`default_origin`](Self::default_origin) supplies the fallback used when
///   neither sets `allowed-origins`.
/// - [`Cors::preset(config)`]: same, starting from a shared [`CorsConfig`]
///   baseline that the file overrides key by key.
/// - [`Cors::permissive()`]: any origin, method and header — a development
///   preset (`allowed_origins(["*"])`, so it wins over the file).
/// - [`Cors::custom(layer)`]: a hand-built [`CorsLayer`], escape hatch; the
///   `cors.*` keys are ignored except `cors.enabled`.
///
/// **Fail-closed.** With no origins from any tier the plugin refuses to
/// boot — except under the `dev` profile, where it falls back to permissive
/// and logs a warning. Set `cors.allowed-origins` or `default_origin(..)`.
///
/// ```ignore
/// AppBuilder::new()
///     .load_config::<AppConfig>()
///     .plugin(Cors::new().default_origin("http://localhost:5173"))
///     .build_state()
///     .await
/// ```
pub struct Cors {
    fixed: Option<CorsLayer>,
    cfg: Layered<CorsConfig>,
}

impl Default for Cors {
    fn default() -> Self {
        Self::new()
    }
}

impl Cors {
    /// A CORS plugin configured by the `cors.*` section (builder setters
    /// override it).
    pub fn new() -> Self {
        Self {
            fixed: None,
            cfg: Layered::new(),
        }
    }

    /// A CORS plugin whose defaults come from `config` — a shared baseline
    /// (a company-wide CORS policy, say) that sits **below** the `cors.*`
    /// section: the app's file overrides it key by key, and the builder
    /// setters override both.
    pub fn preset(config: CorsConfig) -> Self {
        Self {
            fixed: None,
            cfg: Layered::with_preset(config),
        }
    }

    /// A permissive CORS plugin: any origin, method and header. Equivalent to
    /// `Cors::new().allowed_origins(["*"])` — explicit, so it wins over
    /// `cors.allowed-origins`.
    pub fn permissive() -> Self {
        Self::new().allowed_origins(["*"])
    }

    /// A CORS plugin installing a hand-built [`CorsLayer`] as-is. The `cors.*`
    /// keys are not read (only `cors.enabled` still applies).
    pub fn custom(layer: CorsLayer) -> Self {
        Self {
            fixed: Some(layer),
            ..Self::new()
        }
    }

    /// Fallback origin when neither the builder nor `cors.allowed-origins`
    /// sets one — the typical local-frontend default an app ships with.
    pub fn default_origin(self, origin: impl Into<String>) -> Self {
        self.default_origins([origin])
    }

    /// Fallback origins, see [`default_origin`](Self::default_origin). The
    /// preset tier of `allowed-origins`: it replaces the one a
    /// [`preset`](Self::preset) carried.
    pub fn default_origins<I, S>(mut self, origins: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let origins = origins.into_iter().map(Into::into).collect();
        self.cfg.set_preset(|c| c.allowed_origins = Some(origins));
        self
    }

    /// Explicit allowed origins — overrides `cors.allowed-origins`.
    pub fn allowed_origins<I, S>(mut self, origins: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let origins = origins.into_iter().map(Into::into).collect();
        self.cfg.set(|c| c.allowed_origins = Some(origins));
        self
    }

    /// Explicit allowed methods — overrides `cors.allowed-methods`.
    pub fn allowed_methods<I, S>(mut self, methods: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let methods = methods.into_iter().map(Into::into).collect();
        self.cfg.set(|c| c.allowed_methods = Some(methods));
        self
    }

    /// Explicit allowed request headers — overrides `cors.allowed-headers`.
    pub fn allowed_headers<I, S>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let headers = headers.into_iter().map(Into::into).collect();
        self.cfg.set(|c| c.allowed_headers = Some(headers));
        self
    }

    /// Explicit exposed response headers — overrides `cors.expose-headers`.
    pub fn expose_headers<I, S>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let headers = headers.into_iter().map(Into::into).collect();
        self.cfg.set(|c| c.expose_headers = Some(headers));
        self
    }

    /// Explicit `Access-Control-Allow-Credentials` — overrides
    /// `cors.allow-credentials`.
    pub fn allow_credentials(mut self, allow: bool) -> Self {
        self.cfg.set(|c| c.allow_credentials = Some(allow));
        self
    }

    /// Explicit preflight `max-age` — overrides `cors.max-age`.
    pub fn max_age(mut self, max_age: Duration) -> Self {
        self.cfg.set(|c| c.max_age = Some(max_age));
        self
    }

    /// The effective settings: builder > `file` > preset > defaults, validated
    /// once merged. `profile` drives the no-origins fallback (`dev` →
    /// permissive, anything else → error).
    ///
    /// Exposed for tests; `build` is this plus `ctx.add_layer`.
    #[doc(hidden)]
    pub fn resolve_settings(
        &self,
        file: Option<CorsConfig>,
        profile: &str,
    ) -> Result<CorsSettings, PluginBuildError> {
        let cfg = self.cfg.clone().resolve(file);

        let origins = match cfg.allowed_origins {
            Some(origins) => origins,
            None if profile == "dev" => {
                tracing::warn!(
                    target: "r2e::cors",
                    "cors: no allowed origins configured — permissive (any origin) under the `dev` profile; set `cors.allowed-origins`"
                );
                vec!["*".to_string()]
            }
            None => {
                return Err(
                    "cors: `cors.allowed-origins` is not set — set it (a YAML list: \"*\", \
                     exact origins or `scheme://host:*`), or supply a fallback with \
                     `Cors::new().default_origin(..)`"
                        .into(),
                );
            }
        };
        if origins.is_empty() {
            return Err("cors: `cors.allowed-origins` is empty — list at least one origin \
                        (or \"*\")"
                .into());
        }
        for origin in &origins {
            validate_origin(origin).map_err(|why| -> PluginBuildError {
                format!("cors: `cors.allowed-origins` entry rejected — {why}").into()
            })?;
        }
        let any_origin = origins.iter().any(|o| o == "*");

        let methods = cfg
            .allowed_methods
            .map(|names| parse_methods(&names))
            .transpose()?;
        let headers = cfg
            .allowed_headers
            .map(|names| parse_headers("allowed-headers", &names))
            .transpose()?;
        let expose = cfg
            .expose_headers
            .map(|names| parse_headers("expose-headers", &names))
            .transpose()?;
        let allow_credentials = cfg.allow_credentials.unwrap_or(false);
        let max_age = cfg.max_age;

        if allow_credentials {
            // tower-http panics at request time on these combinations
            // (the spec forbids `*` with credentials); refuse at boot instead.
            let conflict = if any_origin {
                Some("`cors.allowed-origins` is \"*\"")
            } else if methods.is_none() {
                Some("`cors.allowed-methods` is unset (= any)")
            } else if headers.is_none() {
                Some("`cors.allowed-headers` is unset (= any)")
            } else {
                None
            };
            if let Some(conflict) = conflict {
                return Err(format!(
                    "cors: `cors.allow-credentials: true` cannot be combined with {conflict} — \
                     browsers reject wildcards on credentialed requests; list explicit values"
                )
                .into());
            }
        }

        Ok(CorsSettings {
            origins,
            methods,
            headers,
            expose,
            allow_credentials,
            max_age,
        })
    }
}

/// The resolved CORS settings (output of [`Cors::resolve_settings`]).
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorsSettings {
    /// Validated origin entries (`*` → any).
    pub origins: Vec<String>,
    /// `None` = any method.
    pub methods: Option<Vec<Method>>,
    /// `None` = any request header.
    pub headers: Option<Vec<HeaderName>>,
    pub expose: Option<Vec<HeaderName>>,
    pub allow_credentials: bool,
    pub max_age: Option<Duration>,
}

impl CorsSettings {
    /// Build the [`CorsLayer`] these settings describe.
    pub fn layer(self) -> CorsLayer {
        let mut layer = CorsLayer::new().allow_origin(allow_origins(&self.origins));
        layer = match self.methods {
            Some(methods) => layer.allow_methods(methods),
            None => layer.allow_methods(cors::Any),
        };
        layer = match self.headers {
            Some(headers) => layer.allow_headers(headers),
            None => layer.allow_headers(cors::Any),
        };
        if let Some(expose) = self.expose {
            layer = layer.expose_headers(expose);
        }
        if self.allow_credentials {
            layer = layer.allow_credentials(true);
        }
        if let Some(max_age) = self.max_age {
            layer = layer.max_age(max_age);
        }
        layer
    }
}

fn parse_methods(names: &[String]) -> Result<Vec<Method>, PluginBuildError> {
    names
        .iter()
        .map(|name| {
            Method::from_bytes(name.trim().to_ascii_uppercase().as_bytes()).map_err(
                |_| -> PluginBuildError {
                    format!("cors: `cors.allowed-methods` contains an invalid method: {name:?}")
                        .into()
                },
            )
        })
        .collect()
}

fn parse_headers(key: &str, names: &[String]) -> Result<Vec<HeaderName>, PluginBuildError> {
    names
        .iter()
        .map(|name| {
            HeaderName::try_from(name.trim()).map_err(|_| -> PluginBuildError {
                format!("cors: `cors.{key}` contains an invalid header name: {name:?}").into()
            })
        })
        .collect()
}

impl Plugin for Cors {
    type Provided = ();
    type Deps = ();
    type Config = CorsConfig;
    type Controllers = ();

    const CONFIG_PREFIX: Option<&'static str> = Some("cors");

    async fn build(
        self,
        _deps: Self::Deps,
        config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        if !ctx.enabled() {
            tracing::debug!(target: "r2e::cors", "cors: disabled by `cors.enabled = false`");
            return Ok(());
        }
        let layer = match self.fixed {
            Some(layer) => layer,
            None => {
                let profile = ctx
                    .config_raw()
                    .and_then(|c| c.try_get::<String>("r2e.profile"))
                    .unwrap_or_else(|| "default".to_string());
                self.resolve_settings(config, &profile)?.layer()
            }
        };
        ctx.add_layer_in(crate::plugin::LayerSlot::Cors, move |router| router.layer(layer));
        Ok(())
    }
}
