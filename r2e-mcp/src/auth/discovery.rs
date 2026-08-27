//! OAuth authorization-server metadata discovery (RFC 8414 / OIDC Discovery).
//!
//! One fetch per TTL window gives every derived surface — the JWKS URL for
//! the validator, the endpoints mirrored by the DCR shim, `scopes_supported`
//! for the PRM document — from the single required config key
//! (`mcp.auth.issuer`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use r2e_core::rt;
use serde_json::Value;

use super::error::McpAuthError;

/// Minimum interval between discovery fetch attempts after a failure
/// (refresh-storm protection, same default as the JWKS cache).
const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// Grace period during which expired metadata may still be served when the
/// IdP is unreachable (stale-if-error).
const MAX_STALE: Duration = Duration::from_secs(3600);

/// Maximum accepted metadata document size (a hostile/broken endpoint must
/// not exhaust memory).
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// The authorization server's metadata document, parsed into the fields R2E
/// uses plus the verbatim document (`raw`) for the DCR shim's mirror routes.
#[derive(Clone, Debug)]
pub struct OAuthServerMetadata {
    /// The complete document as returned by the IdP.
    pub raw: Value,
    /// The `issuer` the document declares (already checked against config).
    pub issuer: String,
    /// `jwks_uri` — required for the `jwt` validation backend.
    pub jwks_uri: Option<String>,
    pub authorization_endpoint: Option<String>,
    pub token_endpoint: Option<String>,
    pub registration_endpoint: Option<String>,
    pub userinfo_endpoint: Option<String>,
    pub introspection_endpoint: Option<String>,
    pub scopes_supported: Vec<String>,
}

impl OAuthServerMetadata {
    /// Parse a metadata document. `raw` must be a JSON object with a string
    /// `issuer`.
    pub fn from_raw(raw: Value) -> Result<Self, McpAuthError> {
        let issuer = raw
            .get("issuer")
            .and_then(Value::as_str)
            .ok_or(McpAuthError::Upstream(
                "authorization server metadata has no `issuer`".into(),
            ))?
            .to_string();
        let s = |key: &str| raw.get(key).and_then(Value::as_str).map(str::to_string);
        Ok(Self {
            issuer,
            jwks_uri: s("jwks_uri"),
            authorization_endpoint: s("authorization_endpoint"),
            token_endpoint: s("token_endpoint"),
            registration_endpoint: s("registration_endpoint"),
            userinfo_endpoint: s("userinfo_endpoint"),
            introspection_endpoint: s("introspection_endpoint"),
            scopes_supported: raw
                .get("scopes_supported")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            raw,
        })
    }

    /// Build metadata without any HTTP fetch (`discovery: off` — every
    /// endpoint explicit in config).
    pub fn from_endpoints(
        issuer: impl Into<String>,
        jwks_uri: Option<String>,
        authorization_endpoint: Option<String>,
        token_endpoint: Option<String>,
        registration_endpoint: Option<String>,
        userinfo_endpoint: Option<String>,
        introspection_endpoint: Option<String>,
    ) -> Self {
        let issuer = issuer.into();
        let mut raw = serde_json::Map::new();
        raw.insert("issuer".into(), Value::String(issuer.clone()));
        let mut put = |key: &str, v: &Option<String>| {
            if let Some(v) = v {
                raw.insert(key.into(), Value::String(v.clone()));
            }
        };
        put("jwks_uri", &jwks_uri);
        put("authorization_endpoint", &authorization_endpoint);
        put("token_endpoint", &token_endpoint);
        put("registration_endpoint", &registration_endpoint);
        put("userinfo_endpoint", &userinfo_endpoint);
        put("introspection_endpoint", &introspection_endpoint);
        Self {
            raw: Value::Object(raw),
            issuer,
            jwks_uri,
            authorization_endpoint,
            token_endpoint,
            registration_endpoint,
            userinfo_endpoint,
            introspection_endpoint,
            scopes_supported: Vec::new(),
        }
    }
}

/// Normalise an issuer for equality comparison: trailing slash dropped
/// (Auth0 issuers carry one, most others don't; RFC 8414 §2 compares the
/// rest byte-for-byte).
pub(crate) fn normalize_issuer(issuer: &str) -> &str {
    issuer.trim_end_matches('/')
}

/// The well-known URLs to probe for a given issuer, in order:
///
/// 1. `{issuer}/.well-known/openid-configuration` (OIDC Discovery — what
///    every mainstream IdP serves),
/// 2. `{issuer}/.well-known/oauth-authorization-server` (RFC 8414 suffix
///    form),
/// 3. RFC 8414 §3.1 path-insertion variants
///    (`{origin}/.well-known/…{path}`) when the issuer has a path component
///    — Entra's `/{tid}/v2.0` issuers need these.
pub(crate) fn candidate_urls(issuer: &str) -> Vec<String> {
    let base = normalize_issuer(issuer);
    let mut urls = vec![
        format!("{base}/.well-known/openid-configuration"),
        format!("{base}/.well-known/oauth-authorization-server"),
    ];
    if let Ok(parsed) = url::Url::parse(base) {
        let path = parsed.path();
        if path != "/" && !path.is_empty() {
            if let Some(origin) = base.strip_suffix(path) {
                urls.push(format!(
                    "{origin}/.well-known/oauth-authorization-server{path}"
                ));
                urls.push(format!("{origin}/.well-known/openid-configuration{path}"));
            }
        }
    }
    urls
}

struct CacheState {
    metadata: Option<Arc<OAuthServerMetadata>>,
    last_refresh: Option<Instant>,
    last_attempt: Option<Instant>,
}

/// Discovery client with TTL cache, single-flight refresh and
/// stale-if-error, sharing the freshness rules of the JWKS cache
/// (`r2e_security::jwks::{is_stale, can_attempt, can_use_stale}`).
pub struct DiscoveryClient {
    client: Option<reqwest::Client>,
    issuer: String,
    ttl: Duration,
    cache: rt::sync::Mutex<CacheState>,
}

impl DiscoveryClient {
    /// `client` should come from
    /// [`r2e_security::build_oauth_http_client`] so timeouts and the
    /// HTTPS-only policy match the JWKS fetcher's.
    pub fn new(client: reqwest::Client, issuer: impl Into<String>, ttl_secs: u64) -> Self {
        Self {
            client: Some(client),
            issuer: issuer.into(),
            ttl: Duration::from_secs(ttl_secs),
            cache: rt::sync::Mutex::new(CacheState {
                metadata: None,
                last_refresh: None,
                last_attempt: None,
            }),
        }
    }

    /// A client that never fetches (`discovery: off`): [`get`](Self::get)
    /// always returns `metadata` (built from the explicit `mcp.auth.*`
    /// endpoint keys).
    pub fn fixed(metadata: OAuthServerMetadata) -> Self {
        Self {
            client: None,
            issuer: metadata.issuer.clone(),
            ttl: Duration::ZERO,
            cache: rt::sync::Mutex::new(CacheState {
                metadata: Some(Arc::new(metadata)),
                last_refresh: None,
                last_attempt: None,
            }),
        }
    }

    /// The configured issuer.
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Pre-populate the cache (tests, `discovery: off` with a shared
    /// client).
    pub async fn prime(&self, metadata: OAuthServerMetadata) {
        let mut cache = self.cache.lock().await;
        cache.metadata = Some(Arc::new(metadata));
        cache.last_refresh = Some(Instant::now());
    }

    /// Get the metadata, fetching (or refreshing) when the TTL elapsed.
    ///
    /// Holding the async mutex across the fetch is the single-flight: a
    /// stampede of concurrent callers on an expired cache produces one HTTP
    /// round trip. On fetch failure, metadata still inside the stale grace
    /// period is served with a warning; otherwise the error propagates
    /// (503 at the layer).
    pub async fn get(&self) -> Result<Arc<OAuthServerMetadata>, McpAuthError> {
        use r2e_security::jwks::{can_attempt, can_use_stale, is_stale};

        let mut cache = self.cache.lock().await;
        if self.client.is_none() {
            // Fixed metadata (`discovery: off`) never expires.
            return cache.metadata.clone().ok_or(McpAuthError::Upstream(
                "discovery is off and no metadata was configured".into(),
            ));
        }
        if let Some(meta) = &cache.metadata {
            if !is_stale(cache.last_refresh, self.ttl) {
                return Ok(meta.clone());
            }
            if !can_attempt(cache.last_attempt, MIN_REFRESH_INTERVAL) {
                // Refreshed-recently-and-failed: don't hammer the IdP.
                if can_use_stale(cache.last_refresh, self.ttl, MAX_STALE) {
                    return Ok(meta.clone());
                }
                return Err(McpAuthError::Upstream(
                    "authorization server metadata expired and refresh is rate-limited".into(),
                ));
            }
        }

        cache.last_attempt = Some(Instant::now());
        match self.fetch().await {
            Ok(meta) => {
                let meta = Arc::new(meta);
                cache.metadata = Some(meta.clone());
                cache.last_refresh = Some(Instant::now());
                Ok(meta)
            }
            Err(err) => {
                if let Some(meta) = &cache.metadata {
                    if can_use_stale(cache.last_refresh, self.ttl, MAX_STALE) {
                        tracing::warn!(
                            issuer = %self.issuer,
                            error = %err.description(),
                            "OAuth discovery refresh failed; serving stale metadata"
                        );
                        return Ok(meta.clone());
                    }
                }
                Err(err)
            }
        }
    }

    /// One probe pass over [`candidate_urls`]; the first URL answering 200
    /// with a JSON object wins. The document's `issuer` must match the
    /// configured one (trailing slash ignored) — a mismatch is an error, not
    /// a fallthrough, because a served-but-wrong document means the config
    /// points at the wrong issuer.
    async fn fetch(&self) -> Result<OAuthServerMetadata, McpAuthError> {
        let mut last_err: Option<String> = None;
        for candidate in candidate_urls(&self.issuer) {
            let Some(client) = &self.client else {
                return Err(McpAuthError::Upstream("discovery is off".into()));
            };
            let response = match client.get(&candidate).send().await {
                Ok(r) => r,
                Err(e) => {
                    last_err = Some(format!("{candidate}: {e}"));
                    continue;
                }
            };
            if !response.status().is_success() {
                last_err = Some(format!("{candidate}: HTTP {}", response.status()));
                continue;
            }
            let body = read_limited(response).await.map_err(|e| {
                McpAuthError::Upstream(format!("discovery response from {candidate}: {e}"))
            })?;
            let raw: Value = serde_json::from_slice(&body).map_err(|e| {
                McpAuthError::Upstream(format!("discovery document at {candidate} is not JSON: {e}"))
            })?;
            let meta = OAuthServerMetadata::from_raw(raw)?;
            if normalize_issuer(&meta.issuer) != normalize_issuer(&self.issuer) {
                return Err(McpAuthError::Upstream(format!(
                    "issuer mismatch: config says `{}` but `{candidate}` declares `{}` — fix \
                     `mcp.auth.issuer` to match the IdP's exact issuer (trailing slash aside)",
                    self.issuer, meta.issuer
                )));
            }
            return Ok(meta);
        }
        Err(McpAuthError::Upstream(format!(
            "OAuth discovery failed for issuer `{}`: no well-known endpoint answered ({})",
            self.issuer,
            last_err.unwrap_or_else(|| "no candidate URLs".into())
        )))
    }
}

/// Read a response body, bounded by [`MAX_BODY_BYTES`].
async fn read_limited(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    if let Some(len) = response.content_length() {
        if len > MAX_BODY_BYTES as u64 {
            return Err(format!("body too large: {len} bytes"));
        }
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(format!("body exceeded {MAX_BODY_BYTES} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
