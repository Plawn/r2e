//! Typed `mcp.auth.*` configuration.

use r2e_core::prelude::{ConfigProperties, FromConfigValue};

/// How the OAuth authorization-server metadata is obtained.
#[derive(serde::Deserialize, FromConfigValue, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoveryMode {
    /// Fetch `{issuer}/.well-known/openid-configuration` at boot; a
    /// misconfigured IdP is a boot failure. The default.
    #[default]
    Eager,
    /// Fetch on first use (compose setups where the IdP starts after the
    /// app). Also the default under the `dev` profile.
    Lazy,
    /// Never fetch: every needed endpoint must be configured explicitly
    /// (`jwks-url`, `authorization-endpoint`, …). Guarantees zero network
    /// I/O from the auth layer outside token introspection.
    Off,
}

/// How bearer tokens are validated.
#[derive(serde::Deserialize, FromConfigValue, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TokenValidationMode {
    /// Local JWT signature validation against the issuer's JWKS. Zero
    /// network per request. The default.
    #[default]
    Jwt,
    /// RFC 7662 token introspection (opaque tokens; needs a confidential
    /// `client-id`/`client-secret`). Not implemented yet (P3).
    Introspection,
    /// OIDC `userinfo` probe (Google-style opaque access tokens; forces
    /// `audience: skip`). Not implemented yet (P3).
    Userinfo,
}

/// Which audiences the token's `aud` claim must contain.
#[derive(serde::Deserialize, FromConfigValue, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AudienceMode {
    /// `aud` must contain the canonical resource URI (RFC 8707). The
    /// default.
    #[default]
    Resource,
    /// `aud` must contain the resource URI OR any of `extra-audiences`
    /// (Auth0 API identifiers, Entra app-ID URIs).
    AnyOf,
    /// `aud` must contain `public-client-id` (IdPs that bind tokens to the
    /// requesting client instead of the resource).
    ClientId,
    /// Do not validate `aud` at all. Any token minted by the issuer for any
    /// service passes — only for flows where the token deliberately carries
    /// no usable audience.
    Skip,
}

/// Typed view of the `mcp.auth.*` config section (presence-based: no
/// section ⇒ unauthenticated endpoint).
///
/// ```yaml
/// mcp:
///   auth:
///     issuer: https://id.example.com/realms/acme
///     public-client-id: mcp-public
///     required-scopes: ["mcp"]
/// ```
///
/// Precedence for each knob: **programmatic builder setting > this file
/// config > default** (same rule as [`McpConfig`](crate::McpConfig)).
#[derive(ConfigProperties, Clone, Debug)]
pub struct McpAuthConfig {
    /// Master switch for the section (default `true`); `false` parses the
    /// section but mounts nothing.
    pub enabled: Option<bool>,

    /// The OAuth issuer URL (the ONE required key). Discovery, JWKS and the
    /// `iss` claim check all derive from it.
    pub issuer: String,

    /// Canonical resource URI of this MCP endpoint (RFC 8707). Default:
    /// `{server.public-url}{mcp.path}`, falling back to the loopback bind
    /// address under dev/test profiles.
    pub resource: Option<String>,
    /// Human-readable resource name advertised in the protected-resource
    /// metadata.
    #[config(key = "resource-name")]
    pub resource_name: Option<String>,

    /// Metadata discovery mode (default `eager`; `lazy` under the `dev`
    /// profile).
    pub discovery: Option<DiscoveryMode>,
    /// How long a discovered metadata document is cached (default 3600).
    #[config(key = "discovery-ttl-secs")]
    pub discovery_ttl_secs: Option<u64>,
    /// Explicit JWKS URL (overrides the discovered `jwks_uri`; required for
    /// `discovery: off` with JWT validation).
    #[config(key = "jwks-url")]
    pub jwks_url: Option<String>,
    /// Explicit authorization endpoint (overrides discovery).
    #[config(key = "authorization-endpoint")]
    pub authorization_endpoint: Option<String>,
    /// Explicit token endpoint (overrides discovery).
    #[config(key = "token-endpoint")]
    pub token_endpoint: Option<String>,
    /// Explicit registration endpoint (overrides discovery; rarely needed —
    /// the shim replaces it).
    #[config(key = "registration-endpoint")]
    pub registration_endpoint: Option<String>,
    /// Explicit `userinfo` endpoint (P3: `token-validation: userinfo`).
    #[config(key = "userinfo-endpoint")]
    pub userinfo_endpoint: Option<String>,
    /// Explicit introspection endpoint (P3: `token-validation:
    /// introspection`).
    #[config(key = "introspection-endpoint")]
    pub introspection_endpoint: Option<String>,

    /// Token validation strategy (default `jwt`).
    #[config(key = "token-validation")]
    pub token_validation: Option<TokenValidationMode>,
    /// Confidential client id for introspection (P3).
    #[config(key = "client-id")]
    pub client_id: Option<String>,
    /// Confidential client secret for introspection (P3).
    #[config(key = "client-secret")]
    pub client_secret: Option<String>,
    /// Accepted JWT signature algorithms (default RS256, ES256, PS256).
    #[config(key = "allowed-algorithms")]
    pub allowed_algorithms: Option<Vec<String>>,
    /// Clock-skew leeway in seconds for `exp`/`nbf` (default 60 — fresh
    /// tokens from a slightly-ahead IdP clock must not be rejected).
    #[config(key = "clock-skew-secs")]
    pub clock_skew_secs: Option<u64>,

    /// Audience validation mode (default `resource`).
    pub audience: Option<AudienceMode>,
    /// Additional accepted audiences for `audience: any-of`.
    #[config(key = "extra-audiences")]
    pub extra_audiences: Option<Vec<String>>,

    /// Scopes advertised in the protected-resource metadata (shown by
    /// clients in the consent UI).
    #[config(key = "scopes-supported")]
    pub scopes_supported: Option<Vec<String>>,
    /// Scopes every MCP request must carry (missing ⇒ HTTP 403
    /// `insufficient_scope`).
    #[config(key = "required-scopes")]
    pub required_scopes: Option<Vec<String>>,
    /// Claim carrying the token's scopes (default: `scope`, falling back to
    /// `scp`; Auth0 RBAC uses `permissions`).
    #[config(key = "scope-claim")]
    pub scope_claim: Option<String>,
    /// Custom claim carrying the caller's roles (read as a string array).
    /// Default: the `roles` claim merged with Keycloak's
    /// `realm_access.roles`.
    #[config(key = "roles-claim")]
    pub roles_claim: Option<String>,
    /// Read Keycloak client roles (`resource_access.<this>.roles`) for the
    /// given client id, merged into the caller's roles.
    #[config(key = "client-roles-for")]
    pub client_roles_for: Option<String>,

    /// Public client id handed out by the DCR shim. Setting it enables the
    /// shim (override with `shim`).
    #[config(key = "public-client-id")]
    pub public_client_id: Option<String>,
    /// Force the DCR shim on/off (default: on iff `public-client-id` is
    /// set).
    pub shim: Option<bool>,
    /// Path of the shim's registration endpoint, relative to `mcp.path`
    /// (default `/oauth/register`).
    #[config(key = "registration-path")]
    pub registration_path: Option<String>,
    /// Allowed `redirect_uris` patterns for the shim (exact match, or a
    /// trailing `*` wildcard). Defaults cover localhost, Claude and the MCP
    /// Inspector.
    #[config(key = "redirect-uri-allowlist")]
    pub redirect_uri_allowlist: Option<Vec<String>>,

    /// Allow an `http://` issuer / JWKS URL (local development only).
    #[config(key = "allow-insecure")]
    pub allow_insecure: Option<bool>,
    /// Filter `tools/list` down to the tools the caller's scopes/roles can
    /// actually call (default `true`).
    #[config(key = "filter-tools")]
    pub filter_tools: Option<bool>,
}
