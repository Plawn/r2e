//! IdP-agnostic OAuth 2.1 resource-server layer for the MCP endpoint.
//!
//! Enabled by the presence of the `mcp.auth` config section (or
//! [`McpServer::with_auth`](crate::McpServer::with_auth)). Two keys are
//! enough for any OIDC IdP:
//!
//! ```yaml
//! server:
//!   public-url: https://api.example.com
//! mcp:
//!   auth:
//!     issuer: https://id.example.com/realms/acme
//!     public-client-id: mcp-public   # optional; enables the DCR shim
//! ```
//!
//! What that buys:
//! - RFC 9728 protected-resource metadata (`/.well-known/oauth-protected-resource`)
//!   so MCP clients (Claude, Inspector) discover the authorization server;
//! - bearer-token validation on every MCP request (`WWW-Authenticate`
//!   challenges per RFC 9110/6750, resource-bound audience per RFC 8707);
//! - a static Dynamic Client Registration shim for IdPs that block anonymous
//!   DCR (Keycloak, Google, Entra) — clients "register" and receive the
//!   pre-configured public client id;
//! - per-tool scope requirements (`#[tool(scopes = "...")]`) and `#[roles]`
//!   over the validated principal.
//!
//! Module map: [`config`] (the `mcp.auth.*` section), [`discovery`] (OIDC
//! metadata probe + cache), [`validator`] ([`McpPrincipal`],
//! [`McpTokenValidator`] — an overridable bean), [`layer`] (the tower layer),
//! [`wellknown`] (PRM), [`shim`] (DCR), [`tools`] (per-tool checks),
//! [`error`] (challenge building).

pub mod config;
pub mod discovery;
pub mod error;
pub mod layer;
pub(crate) mod setup;
pub mod shim;
pub mod tools;
pub mod validator;
pub mod wellknown;

pub use config::{AudienceMode, DiscoveryMode, McpAuthConfig, TokenValidationMode};
pub use discovery::{DiscoveryClient, OAuthServerMetadata};
pub use error::McpAuthError;
pub use layer::McpAuthLayer;
pub use tools::{check_tool, AuthDisabled, ToolRequirements};
pub use validator::{McpPrincipal, McpTokenValidator, ScopePolicy, TokenValidatorBackend};
