use std::sync::Arc;

use r2e_security::JwtClaimsValidator;
use tokio::sync::Semaphore;

use crate::client::ClientRegistry;
use crate::config::OidcServerConfig;
use crate::store::UserStoreErased;
use crate::token::TokenService;

/// Internal shared state for local issuer handlers.
pub(crate) struct OidcState {
    pub token_service: TokenService,
    pub user_store: Box<dyn UserStoreErased>,
    pub client_registry: ClientRegistry,
    pub config: OidcServerConfig,
    pub claims_validator: Arc<JwtClaimsValidator>,
    pub jwks_json: Arc<str>,
    pub discovery_json: Arc<str>,
    pub credential_verification_limiter: Arc<Semaphore>,
}

impl OidcState {
    pub fn new(
        key_pair: Arc<crate::keys::OidcKeyPair>,
        token_service: TokenService,
        user_store: Box<dyn UserStoreErased>,
        client_registry: ClientRegistry,
        config: OidcServerConfig,
        issuer: String,
        claims_validator: Arc<JwtClaimsValidator>,
    ) -> Result<Self, crate::error::OidcError> {
        let jwks_json = serde_json::to_string(&key_pair.jwks_json()).map_err(|e| {
            crate::error::OidcError::Internal(format!("failed to serialize JWKS: {e}"))
        })?;
        let discovery_json = serde_json::to_string(&build_discovery_document(
            &config,
            &issuer,
            !client_registry.is_empty(),
        ))
        .map_err(|e| {
            crate::error::OidcError::Internal(format!(
                "failed to serialize discovery document: {e}"
            ))
        })?;

        Ok(Self {
            token_service,
            user_store,
            client_registry,
            claims_validator,
            jwks_json: Arc::from(jwks_json),
            discovery_json: Arc::from(discovery_json),
            credential_verification_limiter: Arc::new(Semaphore::new(
                config.max_credential_verifications,
            )),
            config,
        })
    }
}

#[derive(serde::Serialize)]
struct DiscoveryDocument {
    issuer: String,
    token_endpoint: String,
    jwks_uri: String,
    userinfo_endpoint: String,
    grant_types_supported: Vec<&'static str>,
    token_endpoint_auth_methods_supported: Vec<&'static str>,
    subject_types_supported: Vec<&'static str>,
    scopes_supported: Vec<&'static str>,
    claims_supported: Vec<&'static str>,
}

fn build_discovery_document(
    config: &OidcServerConfig,
    issuer: &str,
    client_credentials_enabled: bool,
) -> DiscoveryDocument {
    let mut grants = Vec::new();
    if config.password_grant_enabled {
        grants.push("password");
    }
    if client_credentials_enabled {
        grants.push("client_credentials");
    }

    DiscoveryDocument {
        issuer: issuer.to_string(),
        token_endpoint: format!("{issuer}/oauth/token"),
        jwks_uri: format!("{issuer}/.well-known/jwks.json"),
        userinfo_endpoint: format!("{issuer}/userinfo"),
        grant_types_supported: grants,
        token_endpoint_auth_methods_supported: vec!["client_secret_basic", "client_secret_post"],
        subject_types_supported: vec!["public"],
        scopes_supported: vec!["openid", "profile", "email", "roles"],
        claims_supported: vec![
            "sub",
            "iss",
            "aud",
            "iat",
            "exp",
            "email",
            "roles",
            "scope",
            "token_use",
            "principal_type",
            "client_id",
        ],
    }
}
