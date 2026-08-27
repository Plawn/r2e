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
///   allowed-hosts: ["mcp.example.com"]
///   allowed-origins: ["https://claude.ai"]
///   max-request-body-bytes: 4194304
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
