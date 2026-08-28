use std::sync::Arc;

use r2e_core::rt::sync::Semaphore;
use r2e_security::JwtClaimsValidator;

use crate::authorization::AuthorizationCodeStore;
use crate::client::ClientRegistry;
use crate::config::OidcServerConfig;
use crate::store::UserStoreErased;
use crate::token::TokenService;

/// Internal shared state for local issuer handlers.
pub(crate) struct OidcState {
    pub token_service: TokenService,
    pub user_store: Box<dyn UserStoreErased>,
    pub client_registry: ClientRegistry,
    pub authorization_codes: AuthorizationCodeStore,
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
        let jwks_json = r2e_core::json::to_string(&key_pair.jwks_json()).map_err(|e| {
            crate::error::OidcError::Internal(format!("failed to serialize JWKS: {e}"))
        })?;
        let authorization_code_enabled = client_registry.has_public_clients();
        let discovery_json = r2e_core::json::to_string(&build_discovery_document(
            &config,
            &issuer,
            !client_registry.is_empty(),
            authorization_code_enabled,
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
            authorization_codes: AuthorizationCodeStore::new(std::time::Duration::from_secs(
                config.authorization_code_ttl_secs,
            )),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    authorization_endpoint: Option<String>,
    token_endpoint: String,
    jwks_uri: String,
    userinfo_endpoint: String,
    grant_types_supported: Vec<&'static str>,
    token_endpoint_auth_methods_supported: Vec<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    code_challenge_methods_supported: Vec<&'static str>,
    subject_types_supported: Vec<&'static str>,
    scopes_supported: Vec<&'static str>,
    claims_supported: Vec<&'static str>,
}

fn build_discovery_document(
    config: &OidcServerConfig,
    issuer: &str,
    client_credentials_enabled: bool,
    authorization_code_enabled: bool,
) -> DiscoveryDocument {
    let mut grants = Vec::new();
    if config.password_grant_enabled {
        grants.push("password");
    }
    if client_credentials_enabled {
        grants.push("client_credentials");
    }
    if authorization_code_enabled {
        grants.push("authorization_code");
    }

    let mut auth_methods = Vec::new();
    if client_credentials_enabled {
        auth_methods.extend(["client_secret_basic", "client_secret_post"]);
    }
    if authorization_code_enabled || config.password_grant_enabled {
        auth_methods.push("none");
    }

    DiscoveryDocument {
        issuer: issuer.to_string(),
        authorization_endpoint: authorization_code_enabled
            .then(|| format!("{issuer}/oauth/authorize")),
        token_endpoint: format!("{issuer}/oauth/token"),
        jwks_uri: format!("{issuer}/.well-known/jwks.json"),
        userinfo_endpoint: format!("{issuer}/userinfo"),
        grant_types_supported: grants,
        token_endpoint_auth_methods_supported: auth_methods,
        code_challenge_methods_supported: authorization_code_enabled
            .then_some("S256")
            .into_iter()
            .collect(),
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
