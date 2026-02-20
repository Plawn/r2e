//! Embedded OIDC server plugin for R2E.
//!
//! Provides JWT token issuance without an external identity provider.
//! Install as a `PreStatePlugin` and `AuthenticatedUser` works out-of-the-box.
//!
//! # Example
//!
//! ```ignore
//! use r2e::prelude::*;
//! use r2e::r2e_oidc::{OidcServer, InMemoryUserStore, OidcUser};
//!
//! let users = InMemoryUserStore::new()
//!     .add_user("alice", "password123", OidcUser {
//!         sub: "user-1".into(),
//!         email: Some("alice@example.com".into()),
//!         roles: vec!["admin".into()],
//!         ..Default::default()
//!     });
//!
//! let oidc = OidcServer::new()
//!     .with_user_store(users);
//!
//! AppBuilder::new()
//!     .plugin(oidc)
//!     .build_state::<Services, _, _>().await
//!     .register_controller::<UserController>()
//!     .serve("0.0.0.0:3000").await.unwrap();
//! ```

pub mod client;
pub mod config;
pub mod error;
pub mod keys;
pub mod store;
pub mod token;

mod handlers;
mod state;

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use r2e_core::builder::{AppBuilder, NoState};
use r2e_core::type_list::{TAppend, TCons, TNil};
use r2e_core::{DeferredAction, PreStatePlugin};
use r2e_security::{JwtClaimsValidator, SecurityConfig};

pub use client::ClientRegistry;
pub use config::OidcServerConfig;
pub use store::{InMemoryUserStore, OidcUser, UserStore};

/// Embedded OIDC server plugin.
///
/// Generates RSA keys, provides `Arc<JwtClaimsValidator>` to the bean graph,
/// and exposes OAuth 2.0 / OIDC endpoints.
pub struct OidcServer {
    config: OidcServerConfig,
    user_store: Option<Box<dyn store::UserStoreErased>>,
    client_registry: ClientRegistry,
}

impl OidcServer {
    /// Create a new OIDC server with default configuration.
    ///
    /// Defaults: issuer = `http://localhost:3000`, audience = `r2e-app`, TTL = 3600s.
    pub fn new() -> Self {
        Self {
            config: OidcServerConfig::default(),
            user_store: None,
            client_registry: ClientRegistry::new(),
        }
    }

    /// Set the JWT issuer claim.
    pub fn issuer(mut self, issuer: impl Into<String>) -> Self {
        self.config.issuer = issuer.into();
        self
    }

    /// Set the JWT audience claim.
    pub fn audience(mut self, audience: impl Into<String>) -> Self {
        self.config.audience = audience.into();
        self
    }

    /// Set the token time-to-live in seconds.
    pub fn token_ttl(mut self, secs: u64) -> Self {
        self.config.token_ttl_secs = secs;
        self
    }

    /// Set the base path for OIDC endpoints.
    pub fn base_path(mut self, path: impl Into<String>) -> Self {
        self.config.base_path = path.into();
        self
    }

    /// Set the user store (required).
    pub fn with_user_store(mut self, store: impl UserStore) -> Self {
        self.user_store = Some(Box::new(store));
        self
    }

    /// Set the client registry for `client_credentials` grant support.
    pub fn with_client_registry(mut self, registry: ClientRegistry) -> Self {
        self.client_registry = registry;
        self
    }
}

impl Default for OidcServer {
    fn default() -> Self {
        Self::new()
    }
}

impl PreStatePlugin for OidcServer {
    type Provided = Arc<JwtClaimsValidator>;
    type Required = TNil;

    fn install<P, R>(
        self,
        app: AppBuilder<NoState, P, R>,
    ) -> AppBuilder<NoState, TCons<Self::Provided, P>, <R as TAppend<Self::Required>>::Output>
    where
        R: TAppend<Self::Required>,
    {
        // 1. Generate RSA-2048 key pair.
        let key_pair = Arc::new(keys::OidcKeyPair::generate(&self.config.kid));

        // 2. Create JwtClaimsValidator with the public key.
        let security_config = SecurityConfig::new(
            "local", // No remote JWKS URL needed.
            &self.config.issuer,
            &self.config.audience,
        );
        let decoding_key = key_pair.decoding_key();
        let claims_validator = Arc::new(JwtClaimsValidator::new_with_static_key(
            decoding_key,
            security_config,
        ));

        // 3. Build internal OIDC state.
        let oidc_state = Arc::new(state::OidcState {
            key_pair: key_pair.clone(),
            token_service: token::TokenService::new(key_pair, self.config.clone()),
            user_store: self
                .user_store
                .expect("OidcServer: user store is required — call .with_user_store()"),
            client_registry: self.client_registry,
            config: self.config,
            claims_validator: claims_validator.clone(),
        });

        // 4. Provide the validator to the bean graph and register routes via deferred action.
        let base_path = oidc_state.config.base_path.clone();
        app.provide(claims_validator)
            .add_deferred(DeferredAction::new("OidcServer", move |ctx| {
                let oidc_state = oidc_state;
                let base_path = base_path;
                ctx.add_layer(Box::new(move |router| {
                    router.merge(oidc_routes(oidc_state, &base_path))
                }));
            }))
            .with_updated_types()
    }
}

/// Build the OIDC Axum router.
fn oidc_routes(state: Arc<state::OidcState>, base_path: &str) -> Router {
    let router = Router::new()
        .route("/oauth/token", post(handlers::token_handler))
        .route(
            "/.well-known/openid-configuration",
            get(handlers::discovery_handler),
        )
        .route("/.well-known/jwks.json", get(handlers::jwks_handler))
        .route("/userinfo", get(handlers::userinfo_handler))
        .with_state(state);

    if base_path.is_empty() {
        router
    } else {
        Router::new().nest(base_path, router)
    }
}

pub mod prelude {
    //! Re-exports of the most commonly used OIDC types.
    pub use crate::{InMemoryUserStore, OidcServer, OidcUser, UserStore};
}
