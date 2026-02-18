use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{encode, Algorithm, Header};
use serde_json::json;

use crate::config::OidcServerConfig;
use crate::error::OidcError;
use crate::keys::OidcKeyPair;
use crate::store::OidcUser;

/// Service for signing JWT tokens.
pub(crate) struct TokenService {
    key_pair: Arc<OidcKeyPair>,
    config: OidcServerConfig,
}

impl TokenService {
    pub fn new(key_pair: Arc<OidcKeyPair>, config: OidcServerConfig) -> Self {
        Self { key_pair, config }
    }

    /// Issue a JWT for the given user.
    pub fn issue_token(&self, user: &OidcUser) -> Result<String, OidcError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| OidcError::Internal(format!("system clock error: {e}")))?
            .as_secs();

        let exp = now + self.config.token_ttl_secs;

        let mut claims = json!({
            "sub": user.sub,
            "iss": self.config.issuer,
            "aud": self.config.audience,
            "iat": now,
            "exp": exp,
            "roles": user.roles,
        });

        if let Some(email) = &user.email {
            claims["email"] = serde_json::Value::String(email.clone());
        }

        // Merge extra claims, skipping reserved standard claims to prevent forgery.
        const RESERVED: &[&str] = &["sub", "iss", "aud", "iat", "exp", "roles", "email"];
        if let serde_json::Value::Object(map) = &mut claims {
            for (k, v) in &user.extra_claims {
                if RESERVED.contains(&k.as_str()) {
                    tracing::warn!(claim = %k, "Ignoring reserved claim in extra_claims");
                } else {
                    map.insert(k.clone(), v.clone());
                }
            }
        }

        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.config.kid.clone());

        encode(&header, &claims, self.key_pair.encoding_key()).map_err(|e| {
            OidcError::Internal(format!("failed to sign JWT: {e}"))
        })
    }

    /// Issue a JWT for a client_credentials grant (no user, just client_id as sub).
    pub fn issue_client_token(&self, client_id: &str) -> Result<String, OidcError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| OidcError::Internal(format!("system clock error: {e}")))?
            .as_secs();

        let exp = now + self.config.token_ttl_secs;

        let claims = json!({
            "sub": client_id,
            "iss": self.config.issuer,
            "aud": self.config.audience,
            "iat": now,
            "exp": exp,
            "roles": serde_json::Value::Array(vec![]),
        });

        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.config.kid.clone());

        encode(&header, &claims, self.key_pair.encoding_key()).map_err(|e| {
            OidcError::Internal(format!("failed to sign JWT: {e}"))
        })
    }

    pub fn token_ttl_secs(&self) -> u64 {
        self.config.token_ttl_secs
    }
}
