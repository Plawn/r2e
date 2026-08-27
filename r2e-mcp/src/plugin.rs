//! The `McpServer` plugin: mounts the MCP streamable-HTTP endpoint on the
//! app router.

use std::sync::Arc;
use std::time::Duration;

use r2e_core::plugin::{PluginBuildContext, PluginBuildError, PluginSetupContext};
use r2e_core::rt::CancelToken;
use r2e_core::Plugin;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
};

use crate::auth::config::McpAuthConfig;
use crate::auth::layer::McpAuthLayer;
use crate::auth::setup::{build_auth, cors_layer, default_cors_origins, AuthInputs};
use crate::auth::validator::McpTokenValidator;
use crate::config::McpConfig;
use crate::handler::{McpRuntime, R2eMcpHandler, ServerIdentity};
use crate::registry::McpServiceRegistry;

/// Marker type provided by the [`McpServer`] plugin.
///
/// Exists so the plugin can participate in the type-level provision list;
/// users don't reference it directly.
#[derive(Clone)]
pub struct McpMarker;

/// MCP server plugin for R2E.
///
/// Install as a `Plugin` before `build_state()`, then register services after
/// it:
///
/// ```ignore
/// use r2e_mcp::{AppBuilderMcpExt, McpServer};
///
/// AppBuilder::new()
///     .plugin(McpServer::new())
///     .build_state()
///     .await
///     .register_mcp_service::<MathTools>()
///     .serve_auto()
/// ```
///
/// The plugin stores an [`McpServiceRegistry`] (ungated, from `setup()`) that
/// `register_mcp_service` fills with built tool routes, and drains it once
/// when the router is assembled: the accumulated tools are served by one
/// rmcp streamable-HTTP service mounted at `mcp.path` (default `/mcp`).
///
/// # Sharded serving
///
/// One session manager, one dispatch table and one cancellation token are
/// built and shared: the sharded server clones the router per SO_REUSEPORT
/// worker, so every worker serves the same session map and schema cache.
///
/// # Shutdown
///
/// The transport holds a dedicated [`CancelToken`] relayed from the app
/// shutdown token at serve time (and from the shutdown hooks) — cancelling it
/// terminates all active MCP sessions/SSE streams so graceful drain does not
/// hang on long-lived streams.
#[derive(Default)]
pub struct McpServer {
    registry: McpServiceRegistry,
    path: Option<String>,
    name: Option<String>,
    version: Option<String>,
    instructions: Option<String>,
    sse_keep_alive_secs: Option<u64>,
    json_response: Option<bool>,
    stateless: Option<bool>,
    allowed_hosts: Option<Vec<String>>,
    allowed_origins: Option<Vec<String>>,
    max_request_body_bytes: Option<u64>,
    cors_allowed_origins: Option<Vec<String>>,
    auth: Option<McpAuthConfig>,
    token_validator: Option<McpTokenValidator>,
}

impl McpServer {
    /// Create the plugin with defaults (endpoint `/mcp`, stateful sessions).
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the endpoint path (default `/mcp`).
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Override the advertised server name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Override the advertised server version.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Set the usage instructions advertised to clients.
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// SSE keep-alive ping interval in seconds (`0` disables pings).
    pub fn with_sse_keep_alive_secs(mut self, secs: u64) -> Self {
        self.sse_keep_alive_secs = Some(secs);
        self
    }

    /// Prefer `application/json` responses (stateless mode only).
    pub fn json_response(mut self, enabled: bool) -> Self {
        self.json_response = Some(enabled);
        self
    }

    /// Serve statelessly (no MCP sessions).
    pub fn stateless(mut self, enabled: bool) -> Self {
        self.stateless = Some(enabled);
        self
    }

    /// Hostnames accepted in the `Host` header (DNS-rebinding protection;
    /// defaults to loopback only).
    pub fn with_allowed_hosts(mut self, hosts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.allowed_hosts = Some(hosts.into_iter().map(Into::into).collect());
        self
    }

    /// Browser origins accepted in the `Origin` header.
    pub fn with_allowed_origins(
        mut self,
        origins: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.allowed_origins = Some(origins.into_iter().map(Into::into).collect());
        self
    }

    /// Maximum POST body size in bytes (default 4 MiB).
    pub fn with_max_request_body_bytes(mut self, bytes: u64) -> Self {
        self.max_request_body_bytes = Some(bytes);
        self
    }

    /// Origins granted CORS access to the MCP endpoint (overrides
    /// `mcp.cors.allowed-origins`). Entries are exact origins or `host:*`
    /// for any port.
    pub fn with_cors_allowed_origins(
        mut self,
        origins: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.cors_allowed_origins = Some(origins.into_iter().map(Into::into).collect());
        self
    }

    /// Enable the OAuth resource-server layer programmatically (overrides
    /// the `mcp.auth` file section entirely when set).
    pub fn with_auth(mut self, auth: McpAuthConfig) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Replace the token validator (custom backends, test pinning without
    /// `override_bean`). Only used when auth is enabled.
    pub fn with_token_validator(mut self, validator: McpTokenValidator) -> Self {
        self.token_validator = Some(validator);
        self
    }
}

/// Validate the configured endpoint path: absolute, single segment target,
/// no trailing slash, no route captures/wildcards (the MCP endpoint is one
/// literal path serving POST/GET/DELETE).
fn validate_path(path: &str) -> Result<(), PluginBuildError> {
    if !path.starts_with('/') {
        return Err(format!("mcp.path must start with '/': got `{path}`").into());
    }
    if path.len() == 1 || path.ends_with('/') {
        return Err(format!(
            "mcp.path must be a non-root path without a trailing slash: got `{path}`"
        )
        .into());
    }
    if path.contains('{') || path.contains('*') {
        return Err(format!(
            "mcp.path must be a literal path (no `{{param}}` captures or wildcards): got `{path}`"
        )
        .into());
    }
    Ok(())
}

impl Plugin for McpServer {
    /// The real coordination happens via [`McpServiceRegistry`] in plugin
    /// data; `McpMarker` is the type-level placeholder. [`McpTokenValidator`]
    /// is a real bean so tests can pin it (`override_bean`) — auth off
    /// provides the inert [`McpTokenValidator::disabled`].
    type Provided = (McpMarker, McpTokenValidator);
    type Deps = ();
    type Config = McpConfig;
    type Controllers = ();
    const CONFIG_PREFIX: Option<&'static str> = Some("mcp");

    fn setup(&mut self, ctx: &mut PluginSetupContext) {
        // Ungated (the scheduler `TaskRegistryHandle` precedent):
        // `register_mcp_service` must find the registry even with
        // `mcp.enabled = false` — build-time surface effects are dropped when
        // disabled, so the datum is deposited here.
        ctx.store_data(self.registry.clone());
    }

    async fn build(
        self,
        _deps: Self::Deps,
        config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        if !ctx.enabled() {
            tracing::info!("MCP server disabled (mcp.enabled = false); endpoint not mounted");
            return Ok((McpMarker, McpTokenValidator::disabled()));
        }
        let cfg = config.unwrap_or_default();

        // Precedence: builder override > file config > default.
        let path = self.path.or(cfg.path).unwrap_or_else(|| "/mcp".to_string());
        validate_path(&path)?;
        let identity = ServerIdentity {
            name: self
                .name
                .or(cfg.name)
                .unwrap_or_else(|| env!("CARGO_PKG_NAME").to_string()),
            version: self
                .version
                .or(cfg.version)
                .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
            instructions: self.instructions.or(cfg.instructions),
        };
        let sse_keep_alive = match self.sse_keep_alive_secs.or(cfg.sse_keep_alive_secs) {
            Some(0) => None,
            Some(secs) => Some(Duration::from_secs(secs)),
            None => Some(Duration::from_secs(15)),
        };
        let json_response = self.json_response.or(cfg.json_response).unwrap_or(false);
        let stateless = self.stateless.or(cfg.stateless).unwrap_or(false);
        let allowed_hosts = self.allowed_hosts.or(cfg.allowed_hosts);
        let allowed_origins = self
            .allowed_origins
            .or(cfg.allowed_origins)
            .unwrap_or_default();
        let max_request_body_bytes = self
            .max_request_body_bytes
            .or(cfg.max_request_body_bytes)
            .map(|b| b as usize);

        // rmcp's default `Host` allowlist is loopback-only (DNS-rebinding
        // protection): a non-loopback deployment without `mcp.allowed-hosts`
        // would silently 403 every request behind a proxy. Warn at boot.
        if allowed_hosts.is_none() {
            let host = ctx
                .config_raw()
                .and_then(|c| c.try_get::<String>("server.host"))
                .unwrap_or_else(|| "0.0.0.0".to_string());
            let loopback = matches!(host.as_str(), "127.0.0.1" | "::1" | "localhost");
            if !loopback {
                tracing::warn!(
                    host = %host,
                    "mcp.allowed-hosts is not set: the MCP endpoint only accepts \
                     loopback `Host` headers (localhost/127.0.0.1/::1). Set \
                     `mcp.allowed-hosts` to your public hostname(s) for non-local \
                     deployments"
                );
            }
        }

        // ── Auth + CORS ─────────────────────────────────────────────────
        // The resolved profile is written back to `r2e.profile` at load time.
        let profile = ctx
            .config_raw()
            .and_then(|c| c.try_get::<String>("r2e.profile"))
            .unwrap_or_default();
        let auth_cfg = match self.auth.or(cfg.auth) {
            Some(auth) if auth.enabled != Some(false) => Some(auth),
            _ => None,
        };
        let (auth_layer, provided_validator, auth_slot, auth_routes) = match auth_cfg {
            Some(auth) => {
                let artifacts = build_auth(AuthInputs {
                    cfg: auth,
                    mcp_path: &path,
                    profile: &profile,
                    public_url: ctx
                        .config_raw()
                        .and_then(|c| c.try_get::<String>("server.public-url")),
                    server_host: ctx
                        .config_raw()
                        .and_then(|c| c.try_get::<String>("server.host")),
                    server_port: ctx.config_raw().and_then(|c| c.try_get::<u16>("server.port")),
                    allowed_origins: &allowed_origins,
                    validator_override: self.token_validator,
                })
                .await?;
                (
                    artifacts.layer,
                    artifacts.validator,
                    Some(artifacts.slot),
                    Some(artifacts.extra_routes),
                )
            }
            None => (McpAuthLayer::disabled(), McpTokenValidator::disabled(), None, None),
        };
        // The layer reads the validator through the slot, filled from the
        // BEAN CONTEXT after the graph resolves: a test-pinned validator
        // (`override_bean`) is resolved, not captured (plugins.md
        // partial-pins rule). Falling back to the built value keeps this
        // total.
        if let Some(slot) = auth_slot {
            let fallback = provided_validator.clone();
            ctx.after_build(move |dctx| {
                let resolved = dctx
                    .bean_context()
                    .try_get::<McpTokenValidator>()
                    .unwrap_or(fallback);
                let _ = slot.set(resolved);
            });
        }
        let cors = cors_layer(
            self.cors_allowed_origins
                .or(cfg.cors.and_then(|c| c.allowed_origins))
                .unwrap_or_else(|| default_cors_origins(&profile)),
        );

        // Dedicated transport token, relayed from the app shutdown token at
        // serve time (`docs/claude/plugins.md` shutdown-token pattern):
        // cancelling it terminates all active sessions/SSE streams so drain
        // does not hang on long-lived streams. The extra `on_shutdown` cancel
        // covers programmatic stops; hookless exits (dev-reload drop, panic)
        // are covered by the serve-time relay task being dropped with the
        // runtime.
        let mcp_cancel = CancelToken::new();
        let relay = mcp_cancel.clone();
        ctx.on_serve(move |serve_ctx| {
            let app = serve_ctx.shutdown_token();
            serve_ctx.track(async move {
                app.cancelled().await;
                relay.cancel();
            });
        });
        let on_stop = mcp_cancel.clone();
        ctx.on_shutdown(move || on_stop.cancel());

        // Drain point: `wrap_router` runs once at build time, after every
        // `register_mcp_service` call filled the registry, and BEFORE the
        // sharded server clones the router per worker — so the session
        // manager, dispatch table and token below are shared by all workers.
        let registry = self.registry;
        ctx.wrap_router(move |router| {
            let Some(services) = registry.take() else {
                tracing::warn!(
                    "McpServer is installed but no MCP service was registered \
                     (`.register_mcp_service::<T>()`); endpoint not mounted"
                );
                return router;
            };
            let runtime = Arc::new(McpRuntime::build(services, identity));
            tracing::info!(
                path = %path,
                tools = ?runtime.tool_names(),
                "Mounting MCP endpoint"
            );
            let handler = R2eMcpHandler::new(runtime);
            let session_manager = Arc::new(LocalSessionManager::default());
            // #[non_exhaustive] upstream: start from Default and overwrite
            // the fields we own (all pub).
            let mut transport_config = StreamableHttpServerConfig::default();
            transport_config.sse_keep_alive = sse_keep_alive;
            // rmcp's "legacy session mode" = MCP sessions (pre-2026-07-28
            // protocol); `mcp.stateless = true` turns them off.
            transport_config.legacy_session_mode = !stateless;
            transport_config.json_response = json_response;
            // The ONLY tokio-util seam: the field is a `CancellationToken`,
            // filled through `From<CancelToken>` without naming the type.
            transport_config.cancellation_token = mcp_cancel.into();
            transport_config.allowed_origins = allowed_origins;
            if let Some(hosts) = allowed_hosts {
                transport_config.allowed_hosts = hosts;
            }
            if let Some(bytes) = max_request_body_bytes {
                transport_config.max_request_body_bytes = bytes;
            }
            let service = StreamableHttpService::new(
                move || Ok(handler.clone()),
                session_manager,
                transport_config,
            );
            // CORS OUTERMOST: even a 401 from the auth layer must carry the
            // CORS headers or a browser client cannot read the
            // `WWW-Authenticate` challenge. The well-known/shim routes are
            // merged NEXT TO the service — public by design, never behind
            // the auth layer (they answer unauthenticated discovery).
            let service = tower::ServiceBuilder::new()
                .layer(cors)
                .layer(auth_layer)
                .service(service);
            let router = router.route_service(&path, service);
            match auth_routes {
                Some(extra) => router.merge(extra),
                None => router,
            }
        });

        Ok((McpMarker, provided_validator))
    }
}
