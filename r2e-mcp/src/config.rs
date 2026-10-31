//! Typed `mcp.*` configuration.

use crate::auth::McpAuthConfig;
use r2e_core::prelude::ConfigProperties;

/// Typed view of the `mcp.*` config section.
///
/// ```yaml
/// mcp:
///   enabled: true            # read by the plugin framework
///   path: /mcp
///   name: my-app
///   version: 1.2.3
///   instructions: "Call `add` to add numbers."
///   sse-keep-alive-secs: 15  # 0 disables SSE keep-alive pings
///   json-response: false     # prefer application/json replies (stateless mode)
///   stateless: false         # no per-session state; scales horizontally
///   max-protocol-version: "2025-11-25"  # cap the MCP revisions advertised/accepted
///   allowed-hosts: ["mcp.example.com"]
///   allowed-origins: ["https://claude.ai"]
///   max-request-body-bytes: 4194304
///   elicitation-timeout-secs: 300  # how long `McpClient::elicit` waits
///   page-size: 100           # paginate the */list results (unset/0 = one page)
/// ```
///
/// Precedence for each knob: **programmatic builder setting
/// ([`McpServer`](crate::McpServer) methods) > this file config > default**.
#[derive(ConfigProperties, Clone, Debug, Default)]
pub struct McpConfig {
    /// Endpoint path the MCP service is mounted at (default `/mcp`).
    pub path: Option<String>,
    /// Server name advertised in `initialize` (default: `CARGO_PKG_NAME` of
    /// r2e-mcp unless overridden — set it to your app's name).
    pub name: Option<String>,
    /// Server version advertised in `initialize`.
    pub version: Option<String>,
    /// Usage instructions advertised to clients.
    pub instructions: Option<String>,
    /// SSE keep-alive ping interval in seconds; `0` disables pings
    /// (default 15).
    #[config(key = "sse-keep-alive-secs")]
    pub sse_keep_alive_secs: Option<u64>,
    /// Prefer `application/json` responses for simple request/response tools
    /// (only effective with `stateless: true`).
    #[config(key = "json-response")]
    pub json_response: Option<bool>,
    /// Serve every request statelessly (no MCP sessions). Required for
    /// load-balanced deployments without sticky sessions.
    pub stateless: Option<bool>,
    /// Highest MCP protocol revision this server advertises and accepts:
    /// bounds `server/discover`, the `initialize` negotiation and the
    /// per-request `_meta` version gate (`-32022` above the cap). Must be a
    /// revision rmcp knows (`2024-11-05`, `2025-03-26`, `2025-06-18`,
    /// `2025-11-25`, `2026-07-28`); anything else fails at boot. Unset =
    /// every known revision. Independent of `stateless`: a cap below
    /// `2026-07-28` keeps clients on `initialize`, and with `stateless:
    /// false` on `Mcp-Session-Id` sessions.
    #[config(key = "max-protocol-version")]
    pub max_protocol_version: Option<String>,
    /// Hostnames (or `host:port` authorities) accepted in the `Host` header.
    /// Defaults to loopback only (DNS-rebinding protection) — public
    /// deployments MUST list their hostname here.
    #[config(key = "allowed-hosts")]
    pub allowed_hosts: Option<Vec<String>>,
    /// Browser origins accepted in the `Origin` header (empty = validation
    /// off).
    #[config(key = "allowed-origins")]
    pub allowed_origins: Option<Vec<String>>,
    /// Maximum POST body size in bytes (default 4 MiB).
    #[config(key = "max-request-body-bytes")]
    pub max_request_body_bytes: Option<u64>,
    /// How long an elicitation (`McpClient::elicit`) waits for the user's
    /// answer, in seconds (default 300).
    #[config(key = "elicitation-timeout-secs")]
    pub elicitation_timeout_secs: Option<u64>,
    /// Maximum entries per `tools/list`, `resources/list`,
    /// `resources/templates/list` and `prompts/list` page; clients follow
    /// `nextCursor` for the rest. Unset or `0` = every list in one page.
    #[config(key = "page-size")]
    pub page_size: Option<u32>,
    /// Browser CORS policy for the MCP endpoint.
    #[config(section)]
    pub cors: Option<McpCorsConfig>,
    /// OAuth 2.1 resource-server layer (presence-based: no section ⇒
    /// unauthenticated endpoint). See [`McpAuthConfig`].
    #[config(section)]
    pub auth: Option<McpAuthConfig>,
}

/// Typed view of the `mcp.cors.*` section.
///
/// Distinct from `mcp.allowed-origins` (the transport-level Origin
/// *rejection* list): `cors.allowed-origins` is what the endpoint *replies*
/// to browsers in `Access-Control-Allow-Origin`. Defaults to Claude's
/// origins (`https://claude.ai`, `https://claude.com`), plus
/// `http://localhost:*` under the `dev` profile.
#[derive(ConfigProperties, Clone, Debug, Default)]
pub struct McpCorsConfig {
    /// Origins granted CORS access to the MCP endpoint. Entries are exact
    /// origins, or `host:*` to accept any port on that host.
    #[config(key = "allowed-origins")]
    pub allowed_origins: Option<Vec<String>>,
}
