use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey};
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};
use tracing::warn;

use crate::config::SecurityConfig;
use crate::error::SecurityError;

/// Raw JWK structure as returned by a JWKS endpoint.
/// Extra fields are allowed by serde's default behavior; we only capture
/// the fields we need plus a few common ones for future extensibility.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct Jwk {
    /// Key ID
    kid: Option<String>,
    /// Key type (e.g. "RSA")
    kty: String,
    /// Algorithm (e.g. "RS256")
    #[serde(default)]
    alg: Option<String>,
    /// Intended key use (e.g. "sig" or "enc")
    #[serde(default, rename = "use")]
    key_use: Option<String>,
    /// Operations for which the key is intended (e.g. ["verify"])
    #[serde(default)]
    key_ops: Option<Vec<String>>,
    /// RSA modulus (base64url)
    #[serde(default)]
    n: Option<String>,
    /// RSA exponent (base64url)
    #[serde(default)]
    e: Option<String>,
    /// EC / OKP x-coordinate (base64url)
    #[serde(default)]
    x: Option<String>,
    /// EC y-coordinate (base64url)
    #[serde(default)]
    y: Option<String>,
}

/// JWKS response envelope.
#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<Jwk>,
}

/// JWK components used to construct and test cached keys.
///
/// Exposed (doc-hidden) only so integration tests can construct it directly;
/// not part of the public API. The cache itself stores a pre-built
/// `Arc<DecodingKey>` rather than these components.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct CachedJwk {
    pub kty: String,
    /// Algorithm the key is intended for (`alg` in the JWK), if advertised.
    pub alg: Option<String>,
    /// Intended use (`use` in the JWK), if advertised.
    pub key_use: Option<String>,
    /// Allowed operations (`key_ops` in the JWK), if advertised.
    pub key_ops: Option<Vec<String>>,
    pub n: Option<String>,
    pub e: Option<String>,
    pub x: Option<String>,
    pub y: Option<String>,
}

impl CachedJwk {
    /// Build a decoding key, first checking that this JWK is compatible with the
    /// algorithm declared in the token header.
    ///
    /// Defense-in-depth against algorithm confusion when more than one algorithm
    /// family is allowed: the key type (`kty`) must match the token's algorithm
    /// family, and if the JWK advertises its own `alg` it must match exactly.
    #[doc(hidden)]
    pub fn to_decoding_key_checked(
        &self,
        algorithm: Algorithm,
    ) -> Result<DecodingKey, SecurityError> {
        validate_key_metadata(
            &self.kty,
            self.alg.as_deref(),
            self.key_use.as_deref(),
            self.key_ops.as_deref(),
            algorithm,
        )?;
        self.to_decoding_key()
    }

    #[doc(hidden)]
    pub fn to_decoding_key(&self) -> Result<DecodingKey, SecurityError> {
        match self.kty.as_str() {
            "RSA" => {
                let n = self.n.as_deref().ok_or_else(|| {
                    SecurityError::ValidationFailed("RSA key missing 'n' component".into())
                })?;
                let e = self.e.as_deref().ok_or_else(|| {
                    SecurityError::ValidationFailed("RSA key missing 'e' component".into())
                })?;
                DecodingKey::from_rsa_components(n, e).map_err(|err| {
                    SecurityError::ValidationFailed(format!(
                        "Failed to construct RSA decoding key: {err}"
                    ))
                })
            }
            "EC" => {
                let x = self.x.as_deref().ok_or_else(|| {
                    SecurityError::ValidationFailed("EC key missing 'x' component".into())
                })?;
                let y = self.y.as_deref().ok_or_else(|| {
                    SecurityError::ValidationFailed("EC key missing 'y' component".into())
                })?;
                DecodingKey::from_ec_components(x, y).map_err(|err| {
                    SecurityError::ValidationFailed(format!(
                        "Failed to construct EC decoding key: {err}"
                    ))
                })
            }
            "OKP" => {
                let x = self.x.as_deref().ok_or_else(|| {
                    SecurityError::ValidationFailed("OKP key missing 'x' component".into())
                })?;
                DecodingKey::from_ed_components(x).map_err(|err| {
                    SecurityError::ValidationFailed(format!(
                        "Failed to construct EdDSA decoding key: {err}"
                    ))
                })
            }
            other => Err(SecurityError::ValidationFailed(format!(
                "Unsupported key type: {other}"
            ))),
        }
    }
}

/// A JWK whose decoding key was built when the JWKS response was loaded.
///
/// Construction failures are retained as validation errors so a malformed key
/// keeps the same request-time behavior as before instead of silently becoming
/// an unknown `kid` or invalidating every other key in the JWKS response.
struct CachedKey {
    kty: String,
    alg: Option<String>,
    key_use: Option<String>,
    key_ops: Option<Vec<String>>,
    decoding_key: Result<Arc<DecodingKey>, String>,
}

impl CachedKey {
    fn new(jwk: CachedJwk) -> Self {
        let decoding_key = jwk
            .to_decoding_key()
            .map(Arc::new)
            .map_err(validation_error_message);

        Self {
            kty: jwk.kty,
            alg: jwk.alg,
            key_use: jwk.key_use,
            key_ops: jwk.key_ops,
            decoding_key,
        }
    }

    fn decoding_key_checked(
        &self,
        algorithm: Algorithm,
    ) -> Result<Arc<DecodingKey>, SecurityError> {
        validate_key_metadata(
            &self.kty,
            self.alg.as_deref(),
            self.key_use.as_deref(),
            self.key_ops.as_deref(),
            algorithm,
        )?;
        self.decoding_key
            .as_ref()
            .map(Arc::clone)
            .map_err(|message| SecurityError::ValidationFailed(message.clone()))
    }
}

fn validation_error_message(error: SecurityError) -> String {
    match error {
        SecurityError::ValidationFailed(message) => message,
        unexpected => unexpected.to_string(),
    }
}

fn validate_key_metadata(
    kty: &str,
    advertised_algorithm: Option<&str>,
    key_use: Option<&str>,
    key_ops: Option<&[String]>,
    algorithm: Algorithm,
) -> Result<(), SecurityError> {
    let expected_kty = kty_for_algorithm(algorithm);
    if kty != expected_kty {
        return Err(SecurityError::ValidationFailed(format!(
            "JWK key type '{kty}' is incompatible with token algorithm {algorithm:?}"
        )));
    }
    if let Some(advertised_algorithm) = advertised_algorithm {
        if !advertised_algorithm.eq_ignore_ascii_case(&format!("{algorithm:?}")) {
            return Err(SecurityError::ValidationFailed(format!(
                "JWK algorithm '{advertised_algorithm}' does not match token algorithm {algorithm:?}"
            )));
        }
    }
    if let Some(key_use) = key_use {
        if key_use != "sig" {
            return Err(SecurityError::ValidationFailed(format!(
                "JWK use '{key_use}' does not permit signature verification"
            )));
        }
    }
    if let Some(key_ops) = key_ops {
        if !key_ops.iter().any(|operation| operation == "verify") {
            return Err(SecurityError::ValidationFailed(
                "JWK key_ops does not permit signature verification".into(),
            ));
        }
    }
    Ok(())
}

/// Cached state behind the lock.
struct CacheInner {
    keys: HashMap<String, CachedKey>,
    last_refresh: Option<Instant>,
    last_refresh_attempt: Option<Instant>,
}

/// JWKS cache that stores public keys fetched from a JWKS endpoint.
///
/// Keys are indexed by `kid` (Key ID). When a requested `kid` is not found,
/// the cache automatically refreshes from the JWKS endpoint before failing.
pub struct JwksCache {
    inner: Arc<RwLock<CacheInner>>,
    config: SecurityConfig,
    client: reqwest::Client,
    refresh_lock: Mutex<()>,
}

impl JwksCache {
    /// Create a new JWKS cache and perform an initial fetch of keys.
    pub async fn new(config: SecurityConfig) -> Result<Self, SecurityError> {
        validate_jwks_url(&config.jwks_url, config.allow_insecure_jwks_url)?;
        let client = build_jwks_client(&config)?;
        let cache = Self {
            inner: Arc::new(RwLock::new(CacheInner {
                keys: HashMap::new(),
                last_refresh: None,
                last_refresh_attempt: None,
            })),
            config,
            client,
            refresh_lock: Mutex::new(()),
        };
        cache.refresh().await?;
        Ok(cache)
    }

    /// Retrieve the decoding key for the given `kid`.
    ///
    /// If the `kid` is not in the cache, the cache is refreshed first.
    /// If still not found after refresh, returns `SecurityError::UnknownKeyId`.
    ///
    /// `algorithm` is the algorithm declared in the token header; the resolved
    /// JWK must be compatible with it (see [`CachedJwk::to_decoding_key_checked`]).
    pub async fn get_key(
        &self,
        kid: &str,
        algorithm: Algorithm,
    ) -> Result<DecodingKey, SecurityError> {
        self.get_shared_key(kid, algorithm)
            .await
            .map(|key| key.as_ref().clone())
    }

    /// Resolve the cached decoding key without cloning its internal buffers.
    pub(crate) async fn get_shared_key(
        &self,
        kid: &str,
        algorithm: Algorithm,
    ) -> Result<Arc<DecodingKey>, SecurityError> {
        let ttl = Duration::from_secs(self.config.jwks_cache_ttl_secs);
        let max_stale = Duration::from_secs(self.config.jwks_max_stale_secs);

        // First, try cache. If stale or missing, attempt a refresh.
        let (force_refresh, had_cached_key) = {
            let cache = self.inner.read().await;
            if let Some(jwk) = cache.keys.get(kid) {
                if is_stale(cache.last_refresh, ttl) {
                    (false, true)
                } else {
                    return jwk.decoding_key_checked(algorithm);
                }
            } else {
                (true, false)
            }
        };

        // Kid not found (or cache was stale). Attempt refresh, then classify the
        // resulting key by age. A stale key is only usable for a bounded grace
        // period; after that authentication fails closed until refresh succeeds.
        let refresh_result = self.try_refresh(force_refresh).await;
        let cache = self.inner.read().await;
        if let Some(jwk) = cache.keys.get(kid) {
            if !is_stale(cache.last_refresh, ttl) {
                return jwk.decoding_key_checked(algorithm);
            }
            if can_use_stale(cache.last_refresh, ttl, max_stale) {
                match &refresh_result {
                    Err(err) => {
                        warn!(kid, error = %err, "JWKS refresh failed; using stale cached key");
                    }
                    Ok(()) => {
                        warn!(kid, "JWKS refresh deferred; using stale cached key");
                    }
                }
                return jwk.decoding_key_checked(algorithm);
            }
        }
        drop(cache);

        match refresh_result {
            Err(err) => Err(err),
            Ok(()) if had_cached_key => Err(SecurityError::JwksFetchError(format!(
                "Cached JWKS key '{kid}' exceeded the maximum stale grace period"
            ))),
            Ok(()) => Err(SecurityError::UnknownKeyId(kid.to_string())),
        }
    }

    /// Force a refresh of the JWKS cache from the remote endpoint.
    async fn refresh(&self) -> Result<(), SecurityError> {
        let response = self
            .client
            .get(&self.config.jwks_url)
            .send()
            .await
            .map_err(|e| SecurityError::JwksFetchError(e.to_string()))?;

        let response = response
            .error_for_status()
            .map_err(|e| SecurityError::JwksFetchError(e.to_string()))?;

        let body = read_body_limited(response, self.config.jwks_max_response_bytes).await?;

        let jwks: JwksResponse = serde_json::from_slice(&body)
            .map_err(|e| SecurityError::JwksFetchError(format!("Failed to parse JWKS: {e}")))?;

        let mut keys = HashMap::new();
        for jwk in jwks.keys {
            let Some(kid) = jwk.kid else {
                continue;
            };
            let cached = CachedJwk {
                kty: jwk.kty,
                alg: jwk.alg,
                key_use: jwk.key_use,
                key_ops: jwk.key_ops,
                n: jwk.n,
                e: jwk.e,
                x: jwk.x,
                y: jwk.y,
            };
            keys.insert(kid, CachedKey::new(cached));
        }

        let now = Instant::now();
        let mut cache = self.inner.write().await;
        cache.keys = keys;
        cache.last_refresh = Some(now);
        cache.last_refresh_attempt = Some(now);

        Ok(())
    }

    async fn try_refresh(&self, force: bool) -> Result<(), SecurityError> {
        let ttl = Duration::from_secs(self.config.jwks_cache_ttl_secs);
        let min_interval = Duration::from_secs(self.config.jwks_min_refresh_interval_secs);

        {
            let cache = self.inner.read().await;
            if !force && !is_stale(cache.last_refresh, ttl) {
                return Ok(());
            }
            if !can_attempt(cache.last_refresh_attempt, min_interval) {
                return Ok(());
            }
        }

        let _guard = self.refresh_lock.lock().await;

        {
            let cache = self.inner.read().await;
            if !force && !is_stale(cache.last_refresh, ttl) {
                return Ok(());
            }
            if !can_attempt(cache.last_refresh_attempt, min_interval) {
                return Ok(());
            }
        }

        {
            let mut cache = self.inner.write().await;
            cache.last_refresh_attempt = Some(Instant::now());
        }

        self.refresh().await
    }
}

/// The JWK key type (`kty`) expected for a given JWT signing algorithm.
#[doc(hidden)]
pub fn kty_for_algorithm(algorithm: Algorithm) -> &'static str {
    use Algorithm::*;
    match algorithm {
        HS256 | HS384 | HS512 => "oct",
        RS256 | RS384 | RS512 | PS256 | PS384 | PS512 => "RSA",
        ES256 | ES384 => "EC",
        EdDSA => "OKP",
    }
}

/// Reject a non-HTTPS JWKS URL unless insecure fetching is explicitly allowed.
#[doc(hidden)]
pub fn validate_jwks_url(url: &str, allow_insecure: bool) -> Result<(), SecurityError> {
    let parsed = reqwest::Url::parse(url).map_err(|error| {
        SecurityError::JwksFetchError(format!("Invalid JWKS URL '{url}': {error}"))
    })?;

    match parsed.scheme() {
        "https" => Ok(()),
        "http" if allow_insecure => Ok(()),
        "http" => Err(SecurityError::JwksFetchError(format!(
            "Refusing to fetch JWKS over a non-HTTPS URL: {url}. \
             Use https:// or call SecurityConfig::allow_insecure_jwks_url() for local development."
        ))),
        scheme => Err(SecurityError::JwksFetchError(format!(
            "Unsupported JWKS URL scheme '{scheme}': only https is allowed by default, or http with an explicit insecure opt-in"
        ))),
    }
}

/// Build the HTTP client used for JWKS retrieval.
///
/// Exposed for integration tests. HTTPS-only mode applies both to initial
/// requests and every redirect followed by reqwest.
#[doc(hidden)]
pub fn build_jwks_client(config: &SecurityConfig) -> Result<reqwest::Client, SecurityError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(config.jwks_request_timeout_secs))
        .connect_timeout(Duration::from_secs(config.jwks_connect_timeout_secs))
        .https_only(!config.allow_insecure_jwks_url)
        .build()
        .map_err(|error| {
            SecurityError::JwksFetchError(format!("Failed to build HTTP client: {error}"))
        })
}

/// Read a response body, failing if it exceeds `max_bytes`.
///
/// Checks the advertised `Content-Length` up front, then streams chunks so a
/// missing or lying `Content-Length` still cannot exhaust memory.
async fn read_body_limited(
    response: reqwest::Response,
    max_bytes: u64,
) -> Result<Vec<u8>, SecurityError> {
    if let Some(len) = response.content_length() {
        if len > max_bytes {
            return Err(SecurityError::JwksFetchError(format!(
                "JWKS response too large: {len} bytes (max {max_bytes})"
            )));
        }
    }

    let max = max_bytes as usize;
    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| SecurityError::JwksFetchError(e.to_string()))?
    {
        if body.len() + chunk.len() > max {
            return Err(SecurityError::JwksFetchError(format!(
                "JWKS response exceeded max size of {max_bytes} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }

    Ok(body)
}

#[doc(hidden)]
pub fn is_stale(last_refresh: Option<Instant>, ttl: Duration) -> bool {
    match last_refresh {
        None => true,
        Some(ts) => ts.elapsed() >= ttl,
    }
}

#[doc(hidden)]
pub fn can_attempt(last_attempt: Option<Instant>, min_interval: Duration) -> bool {
    match last_attempt {
        None => true,
        Some(ts) => ts.elapsed() >= min_interval,
    }
}

/// Whether an expired cache entry is still within its configured grace period.
#[doc(hidden)]
pub fn can_use_stale(last_refresh: Option<Instant>, ttl: Duration, max_stale: Duration) -> bool {
    match last_refresh {
        None => false,
        Some(ts) => ts.elapsed() <= ttl.saturating_add(max_stale),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_key_reuses_prebuilt_decoding_key() {
        let cached = CachedKey::new(CachedJwk {
            kty: "RSA".into(),
            alg: Some("RS256".into()),
            key_use: Some("sig".into()),
            key_ops: Some(vec!["verify".into()]),
            n: Some("AQ".into()),
            e: Some("AQAB".into()),
            x: None,
            y: None,
        });

        let first = cached.decoding_key_checked(Algorithm::RS256).unwrap();
        let second = cached.decoding_key_checked(Algorithm::RS256).unwrap();

        assert!(Arc::ptr_eq(&first, &second));
    }
}
