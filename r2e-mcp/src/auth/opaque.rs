//! Opaque-token validation backends: RFC 7662 introspection and OIDC
//! `userinfo` (`mcp.auth.token-validation: introspection | userinfo`).
//!
//! Both backends trade the JWT path's zero-network-per-request property for
//! a per-token round trip to the IdP, so both cache validated tokens:
//! positive results for `mcp.auth.opaque-cache-ttl-secs` (capped by the
//! token's own `exp`), rejections for a short fixed window. IdP outages are
//! never cached — they surface as 503 and the next request retries.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use r2e_security::identity::build_authenticated_user;
use r2e_security::StandardClaims;

use super::discovery::DiscoveryClient;
use super::error::McpAuthError;
use super::validator::{hash_token, McpPrincipal, ScopePolicy, TokenValidatorBackend};

/// Default `mcp.auth.opaque-cache-ttl-secs`.
pub const DEFAULT_OPAQUE_CACHE_TTL_SECS: u64 = 60;
/// Default `mcp.auth.opaque-cache-max-entries`.
pub const DEFAULT_OPAQUE_CACHE_MAX_ENTRIES: usize = 1024;
/// How long a rejected token stays rejected without re-asking the IdP.
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(5);

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Token cache ─────────────────────────────────────────────────────────

struct CacheEntry {
    /// The full token, compared on every hit — the hash alone must never
    /// authenticate (a forged token colliding on the 64-bit key would
    /// otherwise inherit the cached principal).
    token: String,
    result: Result<McpPrincipal, &'static str>,
    expires_at: Instant,
}

/// A bounded token-keyed validation cache. All operations are sync (the
/// lock is never held across an await).
struct TokenCache {
    ttl: Duration,
    max_entries: usize,
    entries: Mutex<HashMap<u64, CacheEntry>>,
}

impl TokenCache {
    fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            ttl,
            max_entries: max_entries.max(1),
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, hash: u64, bearer: &str) -> Option<Result<McpPrincipal, McpAuthError>> {
        let entries = self.entries.lock().expect("token cache poisoned");
        let entry = entries.get(&hash)?;
        if entry.token != bearer || entry.expires_at <= Instant::now() {
            return None;
        }
        Some(entry.result.clone().map_err(McpAuthError::InvalidToken))
    }

    fn insert(&self, hash: u64, bearer: &str, result: Result<McpPrincipal, &'static str>, ttl: Duration) {
        let mut entries = self.entries.lock().expect("token cache poisoned");
        if entries.len() >= self.max_entries && !entries.contains_key(&hash) {
            let now = Instant::now();
            entries.retain(|_, e| e.expires_at > now);
            if entries.len() >= self.max_entries {
                // Still full of live entries: drop everything. Crude, but the
                // cache is only an optimization — this bounds memory without
                // an LRU list on what is a low-traffic endpoint.
                entries.clear();
            }
        }
        entries.insert(
            hash,
            CacheEntry {
                token: bearer.to_string(),
                result,
                expires_at: Instant::now() + ttl,
            },
        );
    }

    /// Positive-entry TTL: the configured TTL, capped by the token's `exp`.
    fn positive_ttl(&self, exp: Option<u64>) -> Duration {
        match exp {
            Some(exp) => self
                .ttl
                .min(Duration::from_secs(exp.saturating_sub(unix_now()))),
            None => self.ttl,
        }
    }
}

/// Cache a rejection briefly and return it.
fn deny(
    cache: &TokenCache,
    hash: u64,
    bearer: &str,
    reason: &'static str,
) -> Result<McpPrincipal, McpAuthError> {
    cache.insert(hash, bearer, Err(reason), NEGATIVE_CACHE_TTL);
    Err(McpAuthError::InvalidToken(reason))
}

// ── Introspection (RFC 7662) ────────────────────────────────────────────

/// RFC 7662 token-introspection backend (`token-validation: introspection`).
///
/// POSTs each unseen token to the introspection endpoint with the
/// confidential client's Basic credentials, then applies the same `iss`/
/// `exp`/`aud` checks the JWT backend gets from its validator. The plugin
/// builds this from `mcp.auth.*`; construct it directly (via
/// [`McpTokenValidator::custom`](super::McpTokenValidator::custom)) only for
/// setups the config cannot express.
pub struct IntrospectionBackend {
    client: reqwest::Client,
    discovery: Arc<DiscoveryClient>,
    endpoint_override: Option<String>,
    client_id: String,
    client_secret: String,
    /// Accepted `aud` values (`None` = skip the audience check).
    audiences: Option<Vec<String>>,
    leeway_secs: u64,
    scopes: ScopePolicy,
    cache: TokenCache,
}

impl IntrospectionBackend {
    /// A backend introspecting against `discovery`'s advertised
    /// `introspection_endpoint`, authenticating as the given confidential
    /// client. Defaults: no audience check, 60s leeway, default cache.
    pub fn new(
        client: reqwest::Client,
        discovery: Arc<DiscoveryClient>,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        Self {
            client,
            discovery,
            endpoint_override: None,
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            audiences: None,
            leeway_secs: 60,
            scopes: ScopePolicy::default(),
            cache: TokenCache::new(
                Duration::from_secs(DEFAULT_OPAQUE_CACHE_TTL_SECS),
                DEFAULT_OPAQUE_CACHE_MAX_ENTRIES,
            ),
        }
    }

    /// Use a fixed introspection endpoint instead of the discovered one.
    pub fn with_endpoint(mut self, url: impl Into<String>) -> Self {
        self.endpoint_override = Some(url.into());
        self
    }

    /// Require the token's `aud` to contain at least one of `audiences`.
    pub fn with_audiences(mut self, audiences: impl IntoIterator<Item = String>) -> Self {
        self.audiences = Some(audiences.into_iter().collect());
        self
    }

    /// Clock-skew leeway (seconds) for the `exp` check.
    pub fn with_leeway(mut self, secs: u64) -> Self {
        self.leeway_secs = secs;
        self
    }

    /// How scopes and roles are read out of the introspection response.
    pub fn with_scope_policy(mut self, scopes: ScopePolicy) -> Self {
        self.scopes = scopes;
        self
    }

    /// Cache tuning (positive TTL — still capped by the token's `exp` — and
    /// the entry cap).
    pub fn with_cache(mut self, ttl: Duration, max_entries: usize) -> Self {
        self.cache = TokenCache::new(ttl, max_entries);
        self
    }
}

impl TokenValidatorBackend for IntrospectionBackend {
    async fn validate(&self, bearer: &str) -> Result<McpPrincipal, McpAuthError> {
        let hash = hash_token(bearer);
        if let Some(cached) = self.cache.get(hash, bearer) {
            return cached;
        }

        let endpoint = match &self.endpoint_override {
            Some(url) => url.clone(),
            None => self
                .discovery
                .get()
                .await?
                .introspection_endpoint
                .clone()
                .ok_or_else(|| {
                    McpAuthError::Upstream(format!(
                        "authorization server `{}` advertises no `introspection_endpoint`; \
                         set `mcp.auth.introspection-endpoint` explicitly",
                        self.discovery.issuer()
                    ))
                })?,
        };

        let response = self
            .client
            .post(&endpoint)
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .form(&[("token", bearer), ("token_type_hint", "access_token")])
            .send()
            .await
            .map_err(|e| McpAuthError::Upstream(format!("introspection request failed: {e}")))?;
        if !response.status().is_success() {
            // Includes 401/403: those reject OUR client credentials, not the
            // caller's token — a config problem, so 503 without a challenge.
            return Err(McpAuthError::Upstream(format!(
                "introspection endpoint returned HTTP {} (check `mcp.auth.client-id` / \
                 `mcp.auth.client-secret`)",
                response.status()
            )));
        }
        let body: serde_json::Value = response.json().await.map_err(|e| {
            McpAuthError::Upstream(format!("introspection response is not JSON: {e}"))
        })?;

        if !body.get("active").and_then(|v| v.as_bool()).unwrap_or(false) {
            return deny(&self.cache, hash, bearer, "token is not active");
        }
        let claims: StandardClaims = serde_json::from_value(body).map_err(|e| {
            McpAuthError::Upstream(format!("introspection response is not a claims object: {e}"))
        })?;
        let exp = claims.exp;
        if let Some(exp) = exp {
            if exp.saturating_add(self.leeway_secs) < unix_now() {
                return deny(&self.cache, hash, bearer, "token expired");
            }
        }
        // `iss` is optional in an introspection response; when present it
        // must be the configured issuer.
        if let Some(iss) = &claims.iss {
            if iss != self.discovery.issuer() {
                return deny(&self.cache, hash, bearer, "token issuer mismatch");
            }
        }
        if let Some(accepted) = &self.audiences {
            let ok = claims
                .aud
                .as_ref()
                .is_some_and(|aud| accepted.iter().any(|a| aud.contains(a)));
            if !ok {
                return deny(
                    &self.cache,
                    hash,
                    bearer,
                    "token audience does not include this resource",
                );
            }
        }

        let scope_values: Arc<[String]> = self.scopes.scopes(&claims).into();
        let user = build_authenticated_user(claims, &self.scopes);
        let principal = McpPrincipal {
            user,
            scopes: scope_values,
            token_hash: hash,
        };
        self.cache
            .insert(hash, bearer, Ok(principal.clone()), self.cache.positive_ttl(exp));
        Ok(principal)
    }
}

// ── Userinfo (OIDC) ─────────────────────────────────────────────────────

/// OIDC `userinfo` probe backend (`token-validation: userinfo`) — the shape
/// Google's opaque access tokens need: a token is valid iff the userinfo
/// endpoint accepts it as a bearer.
///
/// The response carries identity claims only — no `aud`, no `exp` — so this
/// backend CANNOT bind tokens to this resource (the plugin forces
/// `audience: skip`) and positive cache entries always use the full
/// configured TTL.
pub struct UserinfoBackend {
    client: reqwest::Client,
    discovery: Arc<DiscoveryClient>,
    endpoint_override: Option<String>,
    scopes: ScopePolicy,
    cache: TokenCache,
}

impl UserinfoBackend {
    /// A backend probing `discovery`'s advertised `userinfo_endpoint`.
    pub fn new(client: reqwest::Client, discovery: Arc<DiscoveryClient>) -> Self {
        Self {
            client,
            discovery,
            endpoint_override: None,
            scopes: ScopePolicy::default(),
            cache: TokenCache::new(
                Duration::from_secs(DEFAULT_OPAQUE_CACHE_TTL_SECS),
                DEFAULT_OPAQUE_CACHE_MAX_ENTRIES,
            ),
        }
    }

    /// Use a fixed userinfo endpoint instead of the discovered one.
    pub fn with_endpoint(mut self, url: impl Into<String>) -> Self {
        self.endpoint_override = Some(url.into());
        self
    }

    /// How scopes and roles are read out of the userinfo response.
    pub fn with_scope_policy(mut self, scopes: ScopePolicy) -> Self {
        self.scopes = scopes;
        self
    }

    /// Cache tuning (positive TTL and the entry cap).
    pub fn with_cache(mut self, ttl: Duration, max_entries: usize) -> Self {
        self.cache = TokenCache::new(ttl, max_entries);
        self
    }
}

impl TokenValidatorBackend for UserinfoBackend {
    async fn validate(&self, bearer: &str) -> Result<McpPrincipal, McpAuthError> {
        let hash = hash_token(bearer);
        if let Some(cached) = self.cache.get(hash, bearer) {
            return cached;
        }

        let endpoint = match &self.endpoint_override {
            Some(url) => url.clone(),
            None => self
                .discovery
                .get()
                .await?
                .userinfo_endpoint
                .clone()
                .ok_or_else(|| {
                    McpAuthError::Upstream(format!(
                        "authorization server `{}` advertises no `userinfo_endpoint`; \
                         set `mcp.auth.userinfo-endpoint` explicitly",
                        self.discovery.issuer()
                    ))
                })?,
        };

        let response = self
            .client
            .get(&endpoint)
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|e| McpAuthError::Upstream(format!("userinfo request failed: {e}")))?;
        let status = response.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return deny(&self.cache, hash, bearer, "token rejected by the userinfo endpoint");
        }
        if !status.is_success() {
            return Err(McpAuthError::Upstream(format!(
                "userinfo endpoint returned HTTP {status}"
            )));
        }
        let claims: StandardClaims = response.json().await.map_err(|e| {
            McpAuthError::Upstream(format!("userinfo response is not a claims object: {e}"))
        })?;
        if claims.sub.is_empty() {
            return deny(&self.cache, hash, bearer, "userinfo response has no subject");
        }

        let scope_values: Arc<[String]> = self.scopes.scopes(&claims).into();
        let user = build_authenticated_user(claims, &self.scopes);
        let principal = McpPrincipal {
            user,
            scopes: scope_values,
            token_hash: hash,
        };
        self.cache
            .insert(hash, bearer, Ok(principal.clone()), self.cache.positive_ttl(None));
        Ok(principal)
    }
}
