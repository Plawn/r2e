//! Embedded OIDC server plugin for R2E.
//!
//! Provides JWT token issuance without an external identity provider.
//! Install as a `PreStatePlugin` and `AuthenticatedUser` works out-of-the-box.
//!
//! # Quick start
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
//!     .build_state().await
//!     .register_controller::<UserController>()
//!     .serve("0.0.0.0:3000").await.unwrap();
//! ```
//!
//! # Hot-reload support
//!
//! With hot-reload (`r2e dev`), `main()` is re-executed on each code patch.
//! Using `OidcServer` directly would regenerate RSA keys every time, invalidating
//! all previously issued tokens.
//!
//! Call [`OidcServer::build()`] once in `setup()` to get an [`OidcRuntime`] — a
//! `Clone`-able handle that preserves keys and state across hot-reload cycles:
//!
//! ```ignore
//! // setup() — called once
//! let oidc = OidcServer::new().with_user_store(users).build();
//!
//! // main(env) — called on each hot-patch
//! AppBuilder::new()
//!     .plugin(oidc.clone()) // same keys, same state
//!     .build_state().await
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

use r2e_core::http::routing::{get, post};
use r2e_core::http::Router;
use r2e_core::{PluginInstallContext, PreStatePlugin};
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

impl OidcServer {
    /// Build the OIDC runtime, performing expensive one-time setup (RSA keygen,
    /// state construction). The returned `OidcRuntime` is `Clone` and can be
    /// reused across hot-reload cycles.
    pub fn build(self) -> OidcRuntime {
        // 1. Generate RSA-2048 key pair.
        let key_pair = Arc::new(keys::OidcKeyPair::generate(&self.config.kid));

        // 2. Create JwtClaimsValidator with the public key.
        let security_config = SecurityConfig::new(
            "local",
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

        let base_path = oidc_state.config.base_path.clone();

        OidcRuntime {
            state: oidc_state,
            claims_validator,
            base_path,
        }
    }
}

/// Pre-built OIDC runtime holding all expensive state (RSA keys, user store,
/// client registry). `Clone`-able and reusable across hot-reload cycles.
///
/// # Usage
///
/// ```ignore
/// // setup() — once
/// let oidc = OidcServer::new().with_user_store(users).build();
///
/// // main(env) — hot-patched, called multiple times
/// AppBuilder::new()
///     .plugin(oidc.clone())
///     .build_state().await
/// ```
#[derive(Clone)]
pub struct OidcRuntime {
    state: Arc<state::OidcState>,
    claims_validator: Arc<JwtClaimsValidator>,
    base_path: String,
}

impl PreStatePlugin for OidcRuntime {
    type Provided = (Arc<JwtClaimsValidator>,);
    type Deps = ();
    type LateDeps = ();
    type Config = ();

    fn install(&mut self, (): (), ctx: &mut PluginInstallContext<'_>) -> (Arc<JwtClaimsValidator>,) {
        // `install` takes `&mut self`; the layer closure needs owned values, so
        // clone the (cheap `Arc`) state and take the base path out.
        let oidc_state = self.state.clone();
        let base_path = std::mem::take(&mut self.base_path);
        ctx.add_layer(move |router| router.merge(oidc_routes(oidc_state, &base_path)));

        (self.claims_validator.clone(),)
    }
}

impl PreStatePlugin for OidcServer {
    type Provided = (Arc<JwtClaimsValidator>,);
    type Deps = ();
    type LateDeps = ();
    type Config = ();

    fn install(&mut self, (): (), ctx: &mut PluginInstallContext<'_>) -> (Arc<JwtClaimsValidator>,) {
        // Take ownership out of `&mut self` (OidcServer: Default) to build the
        // runtime, then delegate to its `install`.
        let mut runtime = std::mem::take(self).build();
        runtime.install((), ctx)
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
    pub use crate::{InMemoryUserStore, OidcRuntime, OidcServer, OidcUser, UserStore};
}
