use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::DecodingKey;
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};

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
    /// RSA modulus (base64url)
    #[serde(default)]
    n: Option<String>,
    /// RSA exponent (base64url)
    #[serde(default)]
    e: Option<String>,
}

/// JWKS response envelope.
#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<Jwk>,
}

/// Internal storage for a cached JWK entry.
/// We store the raw components so we can reconstruct a `DecodingKey` on demand
/// (since `DecodingKey` does not implement `Clone`).
#[derive(Debug, Clone)]
struct CachedJwk {
    kty: String,
    n: Option<String>,
    e: Option<String>,
}

impl CachedJwk {
    fn to_decoding_key(&self) -> Result<DecodingKey, SecurityError> {
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
            other => Err(SecurityError::ValidationFailed(format!(
                "Unsupported key type: {other}"
            ))),
        }
    }
}

/// Cached state behind the lock.
struct CacheInner {
    keys: HashMap<String, CachedJwk>,
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
        let client = reqwest::Client::new();
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
    pub async fn get_key(&self, kid: &str) -> Result<DecodingKey, SecurityError> {
        let ttl = Duration::from_secs(self.config.jwks_cache_ttl_secs);

        // First, try cache. If stale or missing, attempt a refresh.
        let mut needs_refresh = false;
        let mut force_refresh = false;
        {
            let cache = self.inner.read().await;
            if let Some(jwk) = cache.keys.get(kid) {
                if is_stale(cache.last_refresh, ttl) {
                    needs_refresh = true;
                    force_refresh = false;
                } else {
                    return jwk.to_decoding_key();
                }
            } else {
                needs_refresh = true;
                force_refresh = true;
            }
        }

        if needs_refresh {
            // Kid not found (or cache was stale). Attempt refresh, then try again.
            self.try_refresh(force_refresh).await?;
        }

        let cache = self.inner.read().await;
        cache
            .keys
            .get(kid)
            .ok_or_else(|| SecurityError::UnknownKeyId(kid.to_string()))?
            .to_decoding_key()
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

        let jwks: JwksResponse = response
            .json()
            .await
            .map_err(|e| SecurityError::JwksFetchError(format!("Failed to parse JWKS: {e}")))?;

        let mut keys = HashMap::new();
        for jwk in jwks.keys {
            if let Some(kid) = &jwk.kid {
                let cached = CachedJwk {
                    kty: jwk.kty.clone(),
                    n: jwk.n.clone(),
                    e: jwk.e.clone(),
                };
                keys.insert(kid.clone(), cached);
            }
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

fn is_stale(last_refresh: Option<Instant>, ttl: Duration) -> bool {
    match last_refresh {
        None => true,
        Some(ts) => ts.elapsed() >= ttl,
    }
}

fn can_attempt(last_attempt: Option<Instant>, min_interval: Duration) -> bool {
    match last_attempt {
        None => true,
        Some(ts) => ts.elapsed() >= min_interval,
    }
}

#[cfg(test)]
mod tests {
    use super::{can_attempt, is_stale};
    use std::time::{Duration, Instant};

    #[test]
    fn stale_when_never_refreshed() {
        assert!(is_stale(None, Duration::from_secs(60)));
    }

    #[test]
    fn stale_when_ttl_elapsed() {
        let ts = Instant::now() - Duration::from_secs(61);
        assert!(is_stale(Some(ts), Duration::from_secs(60)));
    }

    #[test]
    fn not_stale_before_ttl() {
        let ts = Instant::now() - Duration::from_secs(10);
        assert!(!is_stale(Some(ts), Duration::from_secs(60)));
    }

    #[test]
    fn can_attempt_when_never_attempted() {
        assert!(can_attempt(None, Duration::from_secs(10)));
    }

    #[test]
    fn can_attempt_when_interval_elapsed() {
        let ts = Instant::now() - Duration::from_secs(11);
        assert!(can_attempt(Some(ts), Duration::from_secs(10)));
    }

    #[test]
    fn cannot_attempt_too_soon() {
        let ts = Instant::now() - Duration::from_secs(3);
        assert!(!can_attempt(Some(ts), Duration::from_secs(10)));
    }
}
