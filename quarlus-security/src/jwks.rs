use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use jsonwebtoken::DecodingKey;
use serde::Deserialize;
use tokio::sync::RwLock;

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
}

/// JWKS cache that stores public keys fetched from a JWKS endpoint.
///
/// Keys are indexed by `kid` (Key ID). When a requested `kid` is not found,
/// the cache automatically refreshes from the JWKS endpoint before failing.
pub struct JwksCache {
    inner: Arc<RwLock<CacheInner>>,
    config: SecurityConfig,
    client: reqwest::Client,
}

impl JwksCache {
    /// Create a new JWKS cache and perform an initial fetch of keys.
    pub async fn new(config: SecurityConfig) -> Result<Self, SecurityError> {
        let client = reqwest::Client::new();
        let cache = Self {
            inner: Arc::new(RwLock::new(CacheInner {
                keys: HashMap::new(),
                last_refresh: None,
            })),
            config,
            client,
        };
        cache.refresh().await?;
        Ok(cache)
    }

    /// Retrieve the decoding key for the given `kid`.
    ///
    /// If the `kid` is not in the cache, the cache is refreshed first.
    /// If still not found after refresh, returns `SecurityError::UnknownKeyId`.
    pub async fn get_key(&self, kid: &str) -> Result<DecodingKey, SecurityError> {
        // First, try to read from cache
        {
            let cache = self.inner.read().await;
            if let Some(jwk) = cache.keys.get(kid) {
                return jwk.to_decoding_key();
            }
        }

        // Kid not found, refresh and try again
        self.refresh().await?;

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

        let mut cache = self.inner.write().await;
        cache.keys = keys;
        cache.last_refresh = Some(Instant::now());

        Ok(())
    }
}
