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
use crate::identity::{IdentityState, McpIdentityLayer, RequestScope};
#[cfg(feature = "loopback")]
use crate::loopback::{LoopbackAuth, LoopbackWiring, McpLoopback};
use crate::registry::McpServiceRegistry;
use crate::request_log::McpRequestLog;
use crate::resource_updates::McpResourceUpdates;
use crate::session::{
    erase_init, McpSessionInit, McpSessionStore, McpSessions, SessionBindings, SessionInitFn,
    SessionPersistence, TrackSessionId, TransportStore,
};

/// `mcp.session.store-ttl-secs` when unset.
const DEFAULT_SESSION_STORE_TTL: Duration = Duration::from_secs(30 * 60);

const SESSION_STORE_STATELESS: &str = "McpServer: a session store needs sessions — \
    `mcp.stateless = true` serves none; drop one of the two";

/// Resolves the `session_init` hook bean from the built graph (`None` = no
/// such bean).
type ResolveInit = fn(&BeanContext) -> Option<SessionInitFn>;

/// `(target, message)` of the ERROR events rmcp emits for a benign condition;
/// [`McpServer`]'s `setup` contributes each as a
/// [`Suppress`](r2e_core::Suppress) layer
/// (`PluginSetupContext::tracing_layer`), so the framework subscriber drops
/// them.
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
/// # Session lifetime
///
/// rmcp's session manager owns the transport worker of every HTTP session:
/// it ends a session on `DELETE`, on shutdown, and after
/// `mcp.session.idle-timeout-secs` without JSON-RPC traffic
/// ([`with_session_idle_timeout`](Self::with_session_idle_timeout); `0` =
/// never). A later request carrying the dead `Mcp-Session-Id` is answered
/// `404`. `McpSessions` and the auth layer's session bindings only observe
/// sessions through weak references and are pruned once the worker is gone;
/// they never keep a session alive. Loopback sessions have no worker and no
/// idle timeout.
///
/// With an [`McpSessionStore`] ([`with_session_store`](Self::with_session_store),
/// or an `Arc<dyn McpSessionStore>` bean) a session outlives its worker: the
/// handshake is written at `initialize` and re-armed while the session is
/// active (`mcp.session.store-ttl-secs`, default 30 minutes), and a request
/// naming a session this process does not hold — after a restart, an idle
/// eviction, or on another instance — rebuilds it from the record, bound to
/// the same subject. Loopback sessions are never written. See the "Session
/// persistence" section of the MCP feature guide for what a restored
/// session keeps and loses.
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
    session_idle_timeout: Option<Duration>,
    session_store: Option<Arc<dyn McpSessionStore>>,
    session_store_ttl: Option<Duration>,
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

    /// Close an HTTP session after `timeout` without JSON-RPC traffic
    /// (overrides `mcp.session.idle-timeout-secs`; `Duration::ZERO` = never;
    /// default rmcp's 5 minutes). SSE keep-alive pings do not count as
    /// traffic, so a tool call that stays silent longer than this is cut.
    /// No effect on loopback sessions.
    pub fn with_session_idle_timeout(mut self, timeout: Duration) -> Self {
        self.session_idle_timeout = Some(timeout);
        self
    }

    /// Keep HTTP sessions restorable in `store` (takes precedence over an
    /// `Arc<dyn McpSessionStore>` bean). Incompatible with
    /// [`stateless`](Self::stateless): the plugin fails to build.
    pub fn with_session_store(mut self, store: impl McpSessionStore) -> Self {
        self.session_store = Some(Arc::new(store));
        self
    }

    /// Lifetime of a session record (overrides `mcp.session.store-ttl-secs`;
    /// default 30 minutes): written at `initialize`, re-armed on the first
    /// request past half of it. Must be positive. No effect without a store.
    pub fn with_session_store_ttl(mut self, ttl: Duration) -> Self {
        self.session_store_ttl = Some(ttl);
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

/// `mcp.allowed-hosts`, when set, must name at least one host and no empty
/// entry: rmcp reads an EMPTY list as "Host check off", so `[]` (or a
/// `${VAR}` that resolved to nothing) would silently drop the DNS-rebinding
/// protection instead of tightening it.
fn validate_allowed_hosts(hosts: Option<&[String]>) -> Result<(), PluginBuildError> {
    let Some(hosts) = hosts else {
        return Ok(());
    };
    if hosts.is_empty() {
        return Err(
            "mcp.allowed-hosts must not be empty: an empty list disables the `Host` \
             header check (DNS-rebinding protection) entirely. List the public \
             hostname(s), or remove the key for the loopback-only default"
                .to_string()
                .into(),
        );
    }
    if hosts.iter().any(|host| host.trim().is_empty()) {
        return Err(format!(
            "mcp.allowed-hosts: empty entry in {hosts:?} — every entry must be a \
             hostname or `host:port` authority"
        )
        .into());
    }
    // rmcp compares the `Host` header with each entry byte for byte: a
    // padded entry (a trailing space, a `${VAR}` that expanded with a
    // newline) would boot fine and 403 every request.
    if let Some(padded) = hosts.iter().find(|host| host.trim() != host.as_str()) {
        return Err(format!(
            "mcp.allowed-hosts: entry {padded:?} has leading or trailing whitespace — \
             it would never match a `Host` header; every entry must be exactly a \
             hostname or `host:port` authority"
        )
        .into());
    }
    Ok(())
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
    ///
    /// Feature `loopback` adds [`McpLoopback`] (in-process sessions on this
    /// same endpoint) as a fifth element; the tuple is never named by
    /// consumers (`#[inject]` resolves each bean by type).
    #[cfg(not(feature = "loopback"))]
    type Provided = (
        McpTokenValidator,
        McpResourceUpdates,
        McpSessions,
        McpRegistry,
    );
    #[cfg(feature = "loopback")]
    type Provided = (
        McpTokenValidator,
        McpResourceUpdates,
        McpSessions,
        McpRegistry,
        McpLoopback,
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
        // rmcp's known-benign drain errors (#1185) never reach the framework
        // subscriber's output: `build_state()` installs these with it.
        for (target, message) in RMCP_NOISE {
            ctx.tracing_layer(r2e_core::Suppress::error(*target, *message));
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
            return Ok(provided(
                McpTokenValidator::disabled(),
                self.resource_updates,
                McpSessions::default(),
                McpRegistry::default(),
                #[cfg(feature = "loopback")]
                McpLoopback::unmounted(),
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
        validate_allowed_hosts(allowed_hosts.as_deref())?;
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
        let session_cfg = cfg.session.unwrap_or_default();
        // `None` leaves rmcp's default (300 s); `Some(ZERO)` = never.
        let session_idle_timeout = self
            .session_idle_timeout
            .or(session_cfg.idle_timeout_secs.map(Duration::from_secs));
        let session_store_ttl = self
            .session_store_ttl
            .or(session_cfg.store_ttl_secs.map(Duration::from_secs))
            .unwrap_or(DEFAULT_SESSION_STORE_TTL);
        if session_store_ttl.is_zero() {
            return Err("mcp.session.store-ttl-secs (McpServer::with_session_store_ttl) must be > 0: \
                        a session record needs a lifetime"
                .into());
        }
        if stateless && self.session_store.is_some() {
            return Err(SESSION_STORE_STATELESS.into());
        }
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
        // Member-list visibility filtering: on by default whenever the
        // endpoint knows its caller — `mcp.auth` (`mcp.auth.filter-members:
        // false` opts out) or, below, a service's struct-level identity;
        // meaningless — and off — with neither (nothing to filter against).
        let filter_members = auth_cfg
            .as_ref()
            .map(|auth| auth.filter_members.unwrap_or(true));
        let auth_enabled = auth_cfg.is_some();
        // Session id → session, shared by the auth layer and the identity
        // layer: whichever learns the caller's subject binds the session.
        let bindings = SessionBindings::default();
        let (auth_layer, provided_validator, auth_slot, auth_routes, challenges, required_scopes) =
            match auth_cfg {
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
                    bindings: bindings.clone(),
                })
                .await?;
                (
                    Some(artifacts.layer),
                    artifacts.validator,
                    Some(artifacts.slot),
                    Some(artifacts.extra_routes),
                    Some(artifacts.challenges),
                    Some(artifacts.required_scopes),
                )
            }
            None => (None, McpTokenValidator::disabled(), None, None, None, None),
        };
        // A loopback connect presenting a bearer token validates it through
        // the SAME slot the HTTP layer reads (pinned validators included) and
        // under the same `mcp.auth.required-scopes`.
        #[cfg(feature = "loopback")]
        let loopback_auth = auth_slot
            .clone()
            .zip(required_scopes)
            .map(|(slot, required_scopes)| LoopbackAuth {
                slot,
                required_scopes,
            });
        #[cfg(not(feature = "loopback"))]
        let _ = required_scopes;
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
        // The session store: the builder's, else an `Arc<dyn McpSessionStore>`
        // bean — resolved from the built graph like the token validator, so a
        // test-pinned store (`override_bean`) is the one used.
        let store_slot: Arc<OnceLock<Option<Arc<dyn McpSessionStore>>>> =
            Arc::new(OnceLock::new());
        {
            let sessions_slot = Arc::clone(&sessions_slot);
            let members_slot = Arc::clone(&members_slot);
            let init_slot = Arc::clone(&init_slot);
            let store_slot = Arc::clone(&store_slot);
            let fallback = sessions.clone();
            let members_fallback = members.clone();
            let session_init = self.session_init;
            let session_store = self.session_store;
            ctx.after_build(move |dctx| {
                let beans = dctx.bean_context();
                let _ = sessions_slot.set(beans.try_get::<McpSessions>().unwrap_or(fallback));
                let store = session_store.or_else(|| beans.try_get::<Arc<dyn McpSessionStore>>());
                // Graph-stage hooks cannot fail: the bean case is a panic,
                // the builder case a build error (above).
                if stateless && store.is_some() {
                    panic!("{SESSION_STORE_STATELESS}");
                }
                let _ = store_slot.set(store);
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
        // The loopback bean is provided now and wired in `after_routes`,
        // once the endpoint exists: a `connect` before that is `NotMounted`.
        #[cfg(feature = "loopback")]
        let loopback = McpLoopback::unmounted();
        #[cfg(feature = "loopback")]
        let loopback_bean = loopback.clone();
        ctx.after_routes(move |routes: &mut RoutesContext| {
            // No service registered is a valid deployment: every member then
            // comes from the global registry (or a session), after boot.
            let mut services = registry.take().unwrap_or_default();
            // The services' struct-level request scopes run in one layer on
            // the endpoint, before any message is dispatched — the catalog
            // only dispatches.
            let scopes: Vec<RequestScope> = services
                .iter_mut()
                .filter_map(|service| service.routes.request_scope.take())
                .collect();
            // A struct-level identity is a caller to filter lists against,
            // exactly like an `mcp.auth` principal.
            let filter_members = filter_members.unwrap_or(!scopes.is_empty());
            // Shared by the HTTP identity layer and (feature `loopback`)
            // the in-process connect path: one resolution of the services'
            // request scopes, two inbound transports.
            let identity_state = (!scopes.is_empty()).then(|| {
                Arc::new(IdentityState {
                    scopes,
                    bindings: bindings.clone(),
                    challenges: challenges.clone(),
                })
            });
            let identity_layer = identity_state.clone().map(McpIdentityLayer::new);
            #[cfg(not(feature = "loopback"))]
            let _ = identity_state;
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
                bindings: bindings.clone(),
                init: init_slot.get().cloned(),
                shutdown: mcp_cancel.clone(),
                elicitation_timeout,
                page_size,
                supported_versions,
            });
            // The router owns the wiring (below, through the service
            // factory); the bean only holds a weak handle, so dropping the
            // router releases the state the request scopes capture.
            #[cfg(feature = "loopback")]
            let loopback_mount = loopback.mount(LoopbackWiring {
                endpoint: Arc::clone(&endpoint),
                path: path.clone(),
                stateless,
                identity: identity_state,
                auth: loopback_auth,
                bindings: bindings.clone(),
                shutdown: mcp_cancel.clone(),
            });
            // #[non_exhaustive] upstream: start from Default and overwrite
            // the fields we own (all pub).
            let mut session_manager = LocalSessionManager::default();
            match session_idle_timeout {
                Some(idle) if idle.is_zero() => {
                    session_manager.session_config.keep_alive = None;
                }
                Some(idle) => session_manager.session_config.keep_alive = Some(idle),
                None => {}
            }
            let session_manager = Arc::new(session_manager);
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
            // One persistence per process, like the session manager: the
            // sharded server clones the router per worker.
            let persistence = store_slot
                .get()
                .cloned()
                .flatten()
                .map(|store| Arc::new(SessionPersistence::new(store, session_store_ttl)));
            if let Some(persistence) = &persistence {
                transport_config.session_store =
                    Some(Arc::new(TransportStore(Arc::clone(persistence))));
            }
            let request_log = McpRequestLog {
                limit: transport_config.max_request_body_bytes,
            };
            let service = StreamableHttpService::new(
                move || {
                    #[cfg(feature = "loopback")]
                    let _owned_by_the_router = &loopback_mount;
                    Ok(match &persistence {
                        Some(persistence) => R2eMcpHandler::with_persistence(
                            Arc::clone(&endpoint),
                            Arc::clone(persistence),
                        ),
                        None => R2eMcpHandler::new(Arc::clone(&endpoint)),
                    })
                },
                session_manager,
                transport_config,
            );
            // CORS OUTERMOST: even a 401 from the auth layer must carry the
            // CORS headers or a browser client cannot read the
            // `WWW-Authenticate` challenge. The well-known/shim routes are
            // merged NEXT TO the service — public by design, never behind
            // the auth layer (they answer unauthenticated discovery).
            //
            // The request log sits OUTSIDE the auth layer (it must see
            // rejections and their status); it taps the body rather than
            // reading it, so a request rejected before rmcp reads the body
            // (auth, rmcp's header checks) never has it buffered.
            //
            // The identity layer (the services' struct-level
            // `#[inject(identity)]` / `#[inject(request)]` extraction) sits
            // INSIDE the auth layer: an `McpPrincipal` identity reads the
            // principal the auth layer deposited.
            let service = tower::ServiceBuilder::new()
                .layer(cors)
                .layer(request_log)
                .option_layer(auth_layer)
                .option_layer(identity_layer)
                .layer(TrackSessionId {
                    bindings: bindings.clone(),
                })
                .map_response(r2e_core::http::response::IntoResponse::into_response)
                .service(service);
            let mounted = Routes::new().route_service(&path, service);
            routes.register_routes(match auth_routes {
                Some(extra) => mounted.merge(extra),
                None => mounted,
            });
        });

        Ok(provided(
            provided_validator,
            self.resource_updates,
            sessions,
            members,
            #[cfg(feature = "loopback")]
            loopback_bean,
        ))
    }
}

/// The `Provided` tuple at both return sites, so the feature-gated fifth
/// element is spelled once.
fn provided(
    validator: McpTokenValidator,
    updates: McpResourceUpdates,
    sessions: McpSessions,
    members: McpRegistry,
    #[cfg(feature = "loopback")] loopback: McpLoopback,
) -> <McpServer as Plugin>::Provided {
    (
        validator,
        updates,
        sessions,
        members,
        #[cfg(feature = "loopback")]
        loopback,
    )
}
