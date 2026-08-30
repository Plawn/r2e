//! Token validation: the [`McpPrincipal`], the pluggable
//! [`TokenValidatorBackend`], and the [`McpTokenValidator`] bean.

use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::Arc;

use r2e_core::rt;
use r2e_security::openid::RoleExtractor;
use r2e_security::{AuthenticatedUser, SecurityError, StandardClaims};

use super::error::McpAuthError;

/// The authenticated caller of an MCP request.
///
/// Inserted into the request extensions by the auth layer; tools read it
/// through `#[inject(identity)] user: AuthenticatedUser` (the `user` field)
/// or `ToolCall::extension::<McpPrincipal>()` (scopes included).
#[derive(Clone, Debug)]
pub struct McpPrincipal {
    /// The caller's identity (subject, email, roles) — the same type HTTP
    /// route identity injection uses.
    ///
    /// Behind an `Arc`: exactly one `AuthenticatedUser` is built per
    /// authenticated request, and the auth layer shares it between this
    /// principal and the identity extension rather than deep-copying the
    /// claims tree (including the flattened `extra` map) a second time.
    /// Reads go through `Deref` — `principal.user.sub` still works; take an
    /// owned copy with `(*principal.user).clone()`.
    pub user: Arc<AuthenticatedUser>,
    /// The token's granted scopes, normalized (`scope` string, `scp`
    /// string/array, or the configured `scope-claim`).
    pub scopes: Arc<[String]>,
    /// SipHash of the raw bearer token — a stable cache/correlation key that
    /// never exposes token bytes.
    pub token_hash: u64,
}

impl McpPrincipal {
    /// Whether the caller holds `scope`.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

/// A bearer-token validation strategy.
///
/// Implement this to plug a custom validator into the MCP auth layer
/// (`McpServer::with_token_validator`, or `override_bean` in tests). The
/// built-in backends: local JWT validation (default), and — P3 — RFC 7662
/// introspection and OIDC `userinfo`.
pub trait TokenValidatorBackend: Send + Sync + 'static {
    /// Validate a bearer token (the raw value after `Bearer `), producing
    /// the caller's principal.
    fn validate(
        &self,
        bearer: &str,
    ) -> impl Future<Output = Result<McpPrincipal, McpAuthError>> + Send;
}

/// Object-safe adapter over [`TokenValidatorBackend`].
trait ErasedTokenValidator: Send + Sync {
    fn validate<'a>(
        &'a self,
        bearer: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<McpPrincipal, McpAuthError>> + Send + 'a>>;
}

impl<B: TokenValidatorBackend> ErasedTokenValidator for B {
    fn validate<'a>(
        &'a self,
        bearer: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<McpPrincipal, McpAuthError>> + Send + 'a>> {
        Box::pin(TokenValidatorBackend::validate(self, bearer))
    }
}

/// The token validator **bean** the auth layer resolves from the bean graph.
///
/// Provided by the [`McpServer`](crate::McpServer) plugin (built from
/// `mcp.auth.*`); being a provided bean makes it pinnable:
/// `override_bean(McpTokenValidator::jwt(...))` in tests swaps the validation
/// path with zero network I/O. The layer resolves it **after** the graph is
/// built, so a pin always wins.
#[derive(Clone)]
pub struct McpTokenValidator {
    inner: Option<Arc<dyn ErasedTokenValidator>>,
}

impl McpTokenValidator {
    /// Local JWT validation (signature via JWKS or a static key, `iss`/
    /// `aud`/`exp`/`nbf` checks) — the default backend.
    pub fn jwt(validator: Arc<r2e_security::JwtClaimsValidator>, scopes: ScopePolicy) -> Self {
        Self {
            inner: Some(Arc::new(JwtBackend { validator, scopes })),
        }
    }

    /// JWT validation whose JWKS client is built lazily on the first
    /// request — from the discovered `jwks_uri` (or the one already in
    /// `config`). This is the validator the plugin installs: `build()` does
    /// **zero network I/O** through it, so `discovery: off` plus a pinned
    /// validator keeps tests fully offline, and a `lazy`-discovery app can
    /// boot before its IdP.
    pub fn lazy_jwt(
        config: r2e_security::SecurityConfig,
        discovery: Option<Arc<super::discovery::DiscoveryClient>>,
        scopes: ScopePolicy,
    ) -> Self {
        Self {
            inner: Some(Arc::new(LazyJwtBackend {
                config,
                discovery,
                scopes,
                inner: rt::sync::OnceCell::new(),
            })),
        }
    }

    /// A custom validation backend.
    pub fn custom(backend: impl TokenValidatorBackend) -> Self {
        Self {
            inner: Some(Arc::new(backend)),
        }
    }

    /// The inert validator provided when `mcp.auth` is absent/disabled. The
    /// auth layer never calls it; calling it anyway fails closed.
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    /// Validate a bearer token.
    pub async fn validate(&self, bearer: &str) -> Result<McpPrincipal, McpAuthError> {
        match &self.inner {
            Some(backend) => backend.validate(bearer).await,
            // Fail closed: a disabled validator reached by an enabled layer
            // is a wiring bug, not an open door.
            None => Err(McpAuthError::InvalidToken(
                "token validation is not configured",
            )),
        }
    }
}

impl std::fmt::Debug for McpTokenValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpTokenValidator")
            .field("disabled", &self.inner.is_none())
            .finish()
    }
}

/// How scopes and roles are read out of validated claims.
#[derive(Clone, Debug, Default)]
pub struct ScopePolicy {
    /// Claim carrying the scopes (`mcp.auth.scope-claim`). `None` = the
    /// default ladder: `scope` (string), then `scp` (string or array).
    pub scope_claim: Option<String>,
    /// Custom roles claim (`mcp.auth.roles-claim`), read as a string array
    /// and REPLACING the default role sources.
    pub roles_claim: Option<String>,
    /// Merge Keycloak client roles (`resource_access.<id>.roles`) for this
    /// client id (`mcp.auth.client-roles-for`).
    pub client_roles_for: Option<String>,
}

impl ScopePolicy {
    /// Extract the caller's scopes from validated claims.
    ///
    /// A configured `scope-claim` is authoritative; otherwise `scope`
    /// (space-separated string, RFC 8693), then `scp` (string or array —
    /// the Entra/Okta shape).
    pub fn scopes(&self, claims: &StandardClaims) -> Vec<String> {
        if let Some(claim) = &self.scope_claim {
            return match claim.as_str() {
                "scope" => claims.scopes().map(String::from).collect(),
                other => claims.get(other).map(scope_values).unwrap_or_default(),
            };
        }
        let from_scope: Vec<String> = claims.scopes().map(String::from).collect();
        if !from_scope.is_empty() {
            return from_scope;
        }
        claims.get("scp").map(scope_values).unwrap_or_default()
    }

    /// Extract the caller's roles from validated claims.
    ///
    /// Default: the plain `roles` claim merged with Keycloak realm roles
    /// (`realm_access.roles`) — plus `resource_access.<client-roles-for>.roles`
    /// when configured. A configured `roles-claim` replaces the default
    /// sources entirely.
    pub fn roles(&self, claims: &StandardClaims) -> Vec<String> {
        let mut roles: Vec<String> = match &self.roles_claim {
            Some(claim) => claims.get(claim).map(scope_values).unwrap_or_default(),
            None => {
                let mut base = claims.roles.clone().unwrap_or_default();
                merge_unique(&mut base, claims.realm_roles().iter().cloned());
                base
            }
        };
        if let Some(client_id) = &self.client_roles_for {
            merge_unique(&mut roles, claims.client_roles(client_id).iter().cloned());
        }
        roles
    }
}

impl RoleExtractor for ScopePolicy {
    fn extract_roles(&self, claims: &StandardClaims) -> Vec<String> {
        self.roles(claims)
    }
}

/// Read scope-like values from a claim: a space-separated string or an
/// array of strings.
fn scope_values(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(s) => s.split_whitespace().map(String::from).collect(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    }
}

fn merge_unique(into: &mut Vec<String>, items: impl Iterator<Item = String>) {
    for item in items {
        if !into.contains(&item) {
            into.push(item);
        }
    }
}

/// A stable, non-reversible correlation key for a bearer token.
pub(crate) fn hash_token(bearer: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bearer.hash(&mut hasher);
    hasher.finish()
}

// ── JWT backend ─────────────────────────────────────────────────────────

struct JwtBackend {
    validator: Arc<r2e_security::JwtClaimsValidator>,
    scopes: ScopePolicy,
}

impl TokenValidatorBackend for JwtBackend {
    async fn validate(&self, bearer: &str) -> Result<McpPrincipal, McpAuthError> {
        validate_jwt(&self.validator, &self.scopes, bearer).await
    }
}

/// The shared JWT validation path (used by both the eager and lazy
/// backends).
async fn validate_jwt(
    validator: &r2e_security::JwtClaimsValidator,
    scopes: &ScopePolicy,
    bearer: &str,
) -> Result<McpPrincipal, McpAuthError> {
    let claims: StandardClaims = validator.validate(bearer).await.map_err(|err| match err {
        // IdP outage ≠ bad token: 503, no re-auth challenge.
        e if e.is_server_error() => McpAuthError::Upstream(e.to_string()),
        SecurityError::TokenExpired => McpAuthError::InvalidToken("token expired"),
        _ => McpAuthError::InvalidToken("token validation failed"),
    })?;
    let scope_values: Arc<[String]> = scopes.scopes(&claims).into();
    let user = Arc::new(r2e_security::identity::build_authenticated_user(claims, scopes));
    Ok(McpPrincipal {
        user,
        scopes: scope_values,
        token_hash: hash_token(bearer),
    })
}

// ── Lazy JWT backend ────────────────────────────────────────────────────

/// JWT backend whose `JwksCache` (and, when discovery is on, `jwks_uri`)
/// is resolved on the FIRST validated request instead of at plugin build:
/// `build()` stays free of network I/O, and an IdP that boots after the app
/// (compose setups, `discovery: lazy`) only needs to be up by the time the
/// first token arrives. Initialisation failures are not cached — the next
/// request retries.
struct LazyJwtBackend {
    /// Complete except possibly `jwks_url` (empty ⇒ resolve via discovery).
    config: r2e_security::SecurityConfig,
    discovery: Option<Arc<super::discovery::DiscoveryClient>>,
    scopes: ScopePolicy,
    inner: rt::sync::OnceCell<Arc<r2e_security::JwtClaimsValidator>>,
}

impl TokenValidatorBackend for LazyJwtBackend {
    async fn validate(&self, bearer: &str) -> Result<McpPrincipal, McpAuthError> {
        let validator = self
            .inner
            .get_or_try_init(|| async {
                let mut config = self.config.clone();
                if config.jwks_url.is_empty() {
                    let Some(discovery) = &self.discovery else {
                        return Err(McpAuthError::Upstream(
                            "no JWKS URL: set `mcp.auth.jwks-url` or enable discovery".into(),
                        ));
                    };
                    config.jwks_url = discovery.get().await?.jwks_uri.clone().ok_or_else(|| {
                        McpAuthError::Upstream(format!(
                            "authorization server `{}` advertises no `jwks_uri`; set                              `mcp.auth.jwks-url` explicitly",
                            discovery.issuer()
                        ))
                    })?;
                }
                let cache = r2e_security::JwksCache::new(config.clone())
                    .await
                    .map_err(|e| McpAuthError::Upstream(e.to_string()))?;
                Ok(Arc::new(r2e_security::JwtClaimsValidator::new(
                    Arc::new(cache),
                    config,
                )))
            })
            .await?;
        validate_jwt(validator, &self.scopes, bearer).await
    }
}
