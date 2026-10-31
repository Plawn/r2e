//! The `McpServer` plugin: mounts the MCP streamable-HTTP endpoint on the
//! app router.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use r2e_core::beans::BeanContext;
use r2e_core::http::Routes;
use r2e_core::plugin::{PluginBuildContext, PluginBuildError, PluginSetupContext, RoutesContext};
use r2e_core::rt::CancelToken;
use r2e_core::Plugin;
use rmcp::model::ProtocolVersion;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use crate::auth::config::McpAuthConfig;
use crate::auth::setup::{build_auth, cors_layer, default_cors_origins, AuthInputs};
use crate::auth::validator::McpTokenValidator;
use crate::catalog::{Catalog, CatalogOptions, ServerIdentity};
use crate::config::McpConfig;
use crate::elicitation::DEFAULT_ELICITATION_TIMEOUT;
use crate::global::McpRegistry;
use crate::handler::{Endpoint, R2eMcpHandler};
use crate::registry::McpServiceRegistry;
use crate::resource_updates::McpResourceUpdates;
use crate::session::{erase_init, McpSessionInit, McpSessions, SessionInitFn, TrackSessionId};

/// Resolves the `session_init` hook bean from the built graph (`None` = no
/// such bean).
type ResolveInit = fn(&BeanContext) -> Option<SessionInitFn>;

/// `(target, message)` of the ERROR events rmcp emits for a benign condition;
/// [`McpServer`]'s `setup` registers each as a noise rule
/// ([`r2e_core::suppress_error`]), so the framework subscriber drops them.
///
/// - `rmcp::service` — `failed to send pending response during drain` and
///   `fail to response message` (rmcp ≤ 3.8). In stateless streamable HTTP
///   every request runs its own service over a oneshot channel; when the
///   client disconnects first (a `subscriptions/listen` SSE stream the client
///   closes after its idle period, a cancelled tool call) the service is
///   cancelled, drains, and logs the closed channel at ERROR. Nothing leaks
///   and nothing is lost: the response has no reader. Tracked as #1185.
///
/// An entry goes away once the rmcp release that fixes it becomes r2e-mcp's
/// minimum (the canary test in `tests/server/plugin.rs` fails when rmcp stops
/// emitting it).
#[doc(hidden)]
pub const RMCP_NOISE: &[(&str, &str)] = &[
    (
        "rmcp::service",
        "failed to send pending response during drain",
    ),
    ("rmcp::service", "fail to response message"),
];

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
/// when the router is assembled (`build()`, Routes stage): the accumulated
/// tools are served by one rmcp streamable-HTTP service mounted at
/// `mcp.path` (default `/mcp`).
///
/// # Middleware
///
/// The endpoint (and, with `mcp.auth`, the public well-known routes) is
/// mounted in the **Routes** stage, like a controller route: every Graph
/// `add_layer` — `HttpTrace`, metrics, the app's own CORS/auth layers — and
/// the catch-panic slot wrap `/mcp`. The MCP service carries its own CORS
/// layer around the auth layer, so a 401 challenge stays readable by browser
/// clients.
///
/// **Double CORS.** When the app also installs a CORS layer through
/// `add_layer`, that outer layer answers preflights (`OPTIONS /mcp`) before
/// the MCP one ever sees them. It must then allow the request headers
/// `authorization, content-type, mcp-protocol-version, mcp-session-id,
/// last-event-id` and expose `mcp-session-id, www-authenticate`, or MCP
/// browser clients cannot open or resume a session nor read the auth
/// challenge.
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
    resource_updates: McpResourceUpdates,
    path: Option<String>,
    name: Option<String>,
    version: Option<String>,
    instructions: Option<String>,
    sse_keep_alive_secs: Option<u64>,
    json_response: Option<bool>,
    stateless: Option<bool>,
    max_protocol_version: Option<ProtocolVersion>,
    allowed_hosts: Option<Vec<String>>,
    allowed_origins: Option<Vec<String>>,
    max_request_body_bytes: Option<u64>,
    elicitation_timeout: Option<Duration>,
    page_size: Option<u32>,
    cors_allowed_origins: Option<Vec<String>>,
    auth: Option<McpAuthConfig>,
    token_validator: Option<McpTokenValidator>,
    /// Resolves the [`McpSessionInit`] bean once the graph is built.
    session_init: Option<(&'static str, ResolveInit)>,
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

    /// Cap the MCP protocol revisions this server advertises and accepts
    /// (`mcp.max-protocol-version`): `server/discover` lists the known
    /// revisions up to `version`, `initialize` negotiates within them and a
    /// request whose `_meta` names a newer one is refused with `-32022`.
    /// `version` must be one of [`ProtocolVersion::KNOWN_VERSIONS`]; boot
    /// fails otherwise. Independent of [`stateless`](Self::stateless).
    pub fn max_protocol_version(mut self, version: ProtocolVersion) -> Self {
        self.max_protocol_version = Some(version);
        self
    }

    /// Hostnames accepted in the `Host` header (DNS-rebinding protection;
    /// defaults to loopback only).
    pub fn with_allowed_hosts(
        mut self,
        hosts: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
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

    /// How long an elicitation (`McpClient::elicit`) waits for the user's
    /// answer (overrides `mcp.elicitation-timeout-secs`; default 5 minutes).
    pub fn with_elicitation_timeout(mut self, timeout: Duration) -> Self {
        self.elicitation_timeout = Some(timeout);
        self
    }

    /// Maximum entries per `*/list` page (overrides `mcp.page-size`); `0`
    /// serves every list in one page (the default).
    pub fn with_page_size(mut self, page_size: u32) -> Self {
        self.page_size = Some(page_size);
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

    /// Use an application-owned resource update publisher. The same handle is
    /// also provided as a bean; this override is useful when code needs the
    /// publisher before the bean graph is built.
    pub fn with_resource_updates(mut self, updates: McpResourceUpdates) -> Self {
        self.resource_updates = updates;
        self
    }

    /// Shape each MCP session's member list when it opens: the `T` bean's
    /// [`McpSessionInit::init`] runs once per session, on its first
    /// request, with the caller's identity, and returns the groups to
    /// enable/disable and the session-private members to add.
    ///
    /// `T` is resolved from the bean graph (so tests can `override_bean`
    /// it); a missing bean aborts startup.
    pub fn session_init<T>(mut self) -> Self
    where
        T: McpSessionInit + Clone + Send + Sync + 'static,
    {
        self.session_init = Some((std::any::type_name::<T>(), |beans| {
            beans.try_get::<T>().map(erase_init)
        }));
        self
    }
}

/// Validate the configured endpoint path: absolute, single segment target,
/// no trailing slash, no route captures/wildcards (the MCP endpoint is one
/// literal path serving POST/GET/DELETE).
/// The revisions advertised and accepted: every one rmcp knows, or those up
/// to `mcp.max-protocol-version`. The cap must itself be a known revision —
/// a typo (or a `latest` alias) would otherwise silently cap at the newest
/// known one below it.
fn supported_protocol_versions(
    max: Option<&str>,
) -> Result<&'static [ProtocolVersion], PluginBuildError> {
    let Some(max) = max else {
        return Ok(ProtocolVersion::KNOWN_VERSIONS);
    };
    let Some(known) = ProtocolVersion::KNOWN_VERSIONS
        .iter()
        .find(|v| v.as_str() == max)
    else {
        let known: Vec<&str> = ProtocolVersion::KNOWN_VERSIONS
            .iter()
            .map(ProtocolVersion::as_str)
            .collect();
        return Err(format!(
            "mcp.max-protocol-version must be a known MCP protocol revision ({}): got `{max}`",
            known.join(", ")
        )
        .into());
    };
    Ok(ProtocolVersion::known_up_to(known))
}

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
    /// data. [`McpTokenValidator`] is a real bean so tests can pin it
    /// (`override_bean`) — auth off provides the inert
    /// [`McpTokenValidator::disabled`].
    ///
    /// [`McpSessions`] reaches every live MCP session (push list changes
    /// from a consumer, an admin endpoint, …); [`McpRegistry`] adds and
    /// removes the members every session serves, after boot.
    type Provided = (
        McpTokenValidator,
        McpResourceUpdates,
        McpSessions,
        McpRegistry,
    );
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
        // Process-wide and idempotent: rmcp's known-benign drain errors
        // (#1185) never reach the framework subscriber's output.
        for (target, message) in RMCP_NOISE {
            r2e_core::suppress_error(*target, *message);
        }
    }

    async fn build(
        self,
        _deps: Self::Deps,
        config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        if !ctx.enabled() {
            tracing::info!("MCP server disabled (mcp.enabled = false); endpoint not mounted");
            return Ok((
                McpTokenValidator::disabled(),
                self.resource_updates,
                McpSessions::default(),
                McpRegistry::default(),
            ));
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
        let supported_versions = supported_protocol_versions(
            self.max_protocol_version
                .as_ref()
                .map(|v| v.as_str().to_string())
                .or(cfg.max_protocol_version)
                .as_deref(),
        )?;
        let allowed_hosts = self.allowed_hosts.or(cfg.allowed_hosts);
        let allowed_origins = self
            .allowed_origins
            .or(cfg.allowed_origins)
            .unwrap_or_default();
        let max_request_body_bytes = self
            .max_request_body_bytes
            .or(cfg.max_request_body_bytes)
            .map(|b| b as usize);
        let elicitation_timeout = self
            .elicitation_timeout
            .or(cfg.elicitation_timeout_secs.map(Duration::from_secs))
            .unwrap_or(DEFAULT_ELICITATION_TIMEOUT);
        let page_size = self
            .page_size
            .or(cfg.page_size)
            .filter(|&size| size > 0)
            .map(|size| size as usize);

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
        let auth_cfg = self.auth.or(cfg.auth);
        // Member-list visibility filtering: on by default when auth is
        // enabled (`mcp.auth.filter-members: false` opts out); meaningless —
        // and off — without auth (no principal to filter against).
        let filter_members = auth_cfg
            .as_ref()
            .is_some_and(|auth| auth.filter_members.unwrap_or(true));
        let auth_enabled = auth_cfg.is_some();
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
                    server_port: ctx
                        .config_raw()
                        .and_then(|c| c.try_get::<u16>("server.port")),
                    allowed_origins: &allowed_origins,
                    validator_override: self.token_validator,
                })
                .await?;
                (
                    Some(artifacts.layer),
                    artifacts.validator,
                    Some(artifacts.slot),
                    Some(artifacts.extra_routes),
                )
            }
            None => (None, McpTokenValidator::disabled(), None, None),
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
        // Session-shaping beans, resolved (not captured) from the bean
        // context — the Graph stage runs before the Routes-stage mount reads
        // them.
        let sessions = McpSessions::default();
        let members = McpRegistry::default();
        let sessions_slot: Arc<OnceLock<McpSessions>> = Arc::new(OnceLock::new());
        let members_slot: Arc<OnceLock<McpRegistry>> = Arc::new(OnceLock::new());
        let init_slot: Arc<OnceLock<SessionInitFn>> = Arc::new(OnceLock::new());
        {
            let sessions_slot = Arc::clone(&sessions_slot);
            let members_slot = Arc::clone(&members_slot);
            let init_slot = Arc::clone(&init_slot);
            let fallback = sessions.clone();
            let members_fallback = members.clone();
            let session_init = self.session_init;
            ctx.after_build(move |dctx| {
                let beans = dctx.bean_context();
                let _ = sessions_slot.set(beans.try_get::<McpSessions>().unwrap_or(fallback));
                let _ =
                    members_slot.set(beans.try_get::<McpRegistry>().unwrap_or(members_fallback));
                if let Some((type_name, resolve)) = session_init {
                    let hook = resolve(beans).unwrap_or_else(|| {
                        panic!(
                            "McpServer::session_init::<{type_name}>(): no `{type_name}` bean \
                             is registered — provide or register it before `build_state()`"
                        )
                    });
                    let _ = init_slot.set(hook);
                }
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
            serve_ctx.track_named("mcp shutdown relay", async move {
                app.cancelled().await;
                relay.cancel();
            });
        });
        let on_stop = mcp_cancel.clone();
        ctx.on_shutdown(move || on_stop.cancel());

        // Drain point: the Routes-stage closure below runs once, inside
        // `build()` (`AppBuilder::build_inner`), after every
        // `register_mcp_service` call filled the registry, and BEFORE the
        // sharded server clones the router per worker — so the session
        // manager, dispatch table and token below are shared by all workers.
        // The `after_build` effects above are Graph-stage: they run inside
        // `build_state()` (`AppBuilder::from_pre`), strictly before any
        // Routes-stage effect, so the slots are always filled when this
        // closure reads them.
        //
        // Mounted in the Routes stage (not Finalize), `/mcp` and the
        // well-known routes land INSIDE the app layer stack: every Graph
        // `add_layer` (HttpTrace, metrics, app CORS/auth layers) and the
        // catch-panic slot wrap them like a controller route. The MCP CORS
        // layer below is therefore the INNERMOST CORS when the app installs
        // its own (see the `McpServer` docs for the header lists that app
        // layer must allow), and it still decorates the auth layer's 401
        // because the auth layer sits inside it.
        let registry = self.registry;
        let resource_updates = self.resource_updates.clone();
        let sessions_fallback = sessions.clone();
        let members_fallback = members.clone();
        ctx.after_routes(move |routes: &mut RoutesContext| {
            // No service registered is a valid deployment: every member then
            // comes from the global registry (or a session), after boot.
            let services = registry.take().unwrap_or_default();
            if services.is_empty() {
                tracing::info!(
                    "no MCP service registered (`.register_mcp_service::<T>()`); \
                     the endpoint serves McpRegistry / session members only"
                );
            }
            let catalog = Arc::new(Catalog::build(
                services,
                identity,
                CatalogOptions {
                    filter_members,
                    auth_enabled,
                    stateful: !stateless,
                },
            ));
            tracing::info!(
                path = %path,
                tools = ?catalog.tool_names(),
                resources = catalog.resource_count(),
                prompts = catalog.prompt_count(),
                groups = ?catalog.group_names(),
                "Mounting MCP endpoint"
            );
            let sessions = sessions_slot.get().cloned().unwrap_or(sessions_fallback);
            let members = members_slot.get().cloned().unwrap_or(members_fallback);
            members.attach(Arc::clone(&catalog), sessions.clone());
            let endpoint = Arc::new(Endpoint {
                catalog,
                registry: members,
                updates: resource_updates,
                sessions,
                init: init_slot.get().cloned(),
                shutdown: mcp_cancel.clone(),
                elicitation_timeout,
                page_size,
                supported_versions,
            });
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
            transport_config.cancellation_token = mcp_cancel.clone().into();
            transport_config.allowed_origins = allowed_origins;
            if let Some(hosts) = allowed_hosts {
                transport_config.allowed_hosts = hosts;
            }
            if let Some(bytes) = max_request_body_bytes {
                transport_config.max_request_body_bytes = bytes;
            }
            let service = StreamableHttpService::new(
                move || Ok(R2eMcpHandler::new(Arc::clone(&endpoint))),
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
                .option_layer(auth_layer)
                .layer(TrackSessionId)
                .map_response(r2e_core::http::response::IntoResponse::into_response)
                .service(service);
            let mounted = Routes::new().route_service(&path, service);
            routes.register_routes(match auth_routes {
                Some(extra) => mounted.merge(extra),
                None => mounted,
            });
        });

        Ok((provided_validator, self.resource_updates, sessions, members))
    }
}
