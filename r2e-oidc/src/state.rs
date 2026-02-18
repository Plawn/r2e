use std::sync::Arc;

use r2e_security::JwtClaimsValidator;

use crate::client::ClientRegistry;
use crate::config::OidcServerConfig;
use crate::keys::OidcKeyPair;
use crate::store::UserStoreErased;
use crate::token::TokenService;

/// Internal shared state for OIDC server handlers.
pub(crate) struct OidcState {
    pub key_pair: Arc<OidcKeyPair>,
    pub token_service: TokenService,
    pub user_store: Box<dyn UserStoreErased>,
    pub client_registry: ClientRegistry,
    pub config: OidcServerConfig,
    pub claims_validator: Arc<JwtClaimsValidator>,
}

impl OidcState {
    /// Returns the JWKS JSON as a serde_json::Value (owned, no lifetime issues).
    pub fn jwks_json_value(&self) -> serde_json::Value {
        serde_json::to_value(self.key_pair.jwks_json()).expect("failed to serialize JWKS")
    }
}
