use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{encode, Algorithm, Header};
use serde::{Deserialize, Serialize};

use crate::config::OidcServerConfig;
use crate::error::OidcError;
use crate::keys::OidcKeyPair;
use crate::store::OidcUser;

/// Service for signing JWT tokens.
pub(crate) struct TokenService {
    key_pair: Arc<OidcKeyPair>,
    config: OidcServerConfig,
    issuer: String,
}

impl TokenService {
    pub fn new(key_pair: Arc<OidcKeyPair>, config: OidcServerConfig, issuer: String) -> Self {
        Self {
            key_pair,
            config,
            issuer,
        }
    }

    /// Issue a JWT for the given user.
    pub fn issue_user_token(&self, user: &OidcUser, scope: &str) -> Result<String, OidcError> {
        let (iat, exp) = self.timestamps()?;

        let claims = AccessTokenClaims {
            sub: user.sub.clone(),
            iss: self.issuer.clone(),
            aud: self.config.audience.clone(),
            iat,
            exp,
            roles: user.roles.clone(),
            email: user.email.clone(),
            scope: scope.to_string(),
            token_use: "access".into(),
            principal_type: "user".into(),
            client_id: None,
            extra: filter_extra_claims(&user.extra_claims),
        };

        self.sign(&claims)
    }

    /// Issue a JWT for a client_credentials grant.
    pub fn issue_client_token(&self, client_id: &str, scope: &str) -> Result<String, OidcError> {
        let (iat, exp) = self.timestamps()?;

        let claims = AccessTokenClaims {
            sub: format!("client:{client_id}"),
            iss: self.issuer.clone(),
            aud: self.config.audience.clone(),
            iat,
            exp,
            roles: Vec::new(),
            email: None,
            scope: scope.to_string(),
            token_use: "access".into(),
            principal_type: "client".into(),
            client_id: Some(client_id.to_string()),
            extra: Default::default(),
        };

        self.sign(&claims)
    }

    fn timestamps(&self) -> Result<(u64, u64), OidcError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| OidcError::Internal(format!("system clock error: {e}")))?
            .as_secs();

        let exp = now
            .checked_add(self.config.token_ttl_secs)
            .ok_or_else(|| OidcError::Configuration("token TTL overflows exp claim".into()))?;
        Ok((now, exp))
    }

    fn sign(&self, claims: &AccessTokenClaims) -> Result<String, OidcError> {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.key_pair.kid().to_string());

        encode(&header, &claims, self.key_pair.encoding_key())
            .map_err(|e| OidcError::Internal(format!("failed to sign JWT: {e}")))
    }

    pub fn token_ttl_secs(&self) -> u64 {
        self.config.token_ttl_secs
    }
}

/// Claims issued by this embedded access-token issuer.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AccessTokenClaims {
    pub sub: String,
    pub iss: String,
    pub aud: String,
    pub iat: u64,
    pub exp: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    pub token_use: String,
    pub principal_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

impl r2e_security::jwt::JwtClaimSet for AccessTokenClaims {
    fn subject(&self) -> Option<&str> {
        Some(&self.sub)
    }
}

pub(crate) const DEFAULT_USER_SCOPE: &str = "openid profile email roles";

pub(crate) fn normalize_scope(scope: Option<&str>, default_scope: &str) -> String {
    let mut scopes = scope
        .unwrap_or(default_scope)
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    scopes.sort_unstable();
    scopes.dedup();
    scopes.join(" ")
}

pub(crate) fn has_scope(scope: &str, required: &str) -> bool {
    scope
        .split_whitespace()
        .any(|candidate| candidate == required)
}

pub(crate) fn filter_extra_claims(
    extra_claims: &std::collections::HashMap<String, serde_json::Value>,
) -> std::collections::HashMap<String, serde_json::Value> {
    const RESERVED: &[&str] = &[
        "sub",
        "iss",
        "aud",
        "iat",
        "exp",
        "nbf",
        "jti",
        "roles",
        "email",
        "scope",
        "token_use",
        "principal_type",
        "client_id",
    ];

    extra_claims
        .iter()
        .filter_map(|(k, v)| {
            if RESERVED.contains(&k.as_str()) {
                tracing::warn!(claim = %k, "Ignoring reserved claim in extra_claims");
                None
            } else {
                Some((k.clone(), v.clone()))
            }
        })
        .collect()
}
