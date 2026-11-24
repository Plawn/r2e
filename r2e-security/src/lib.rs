//! JWT / OIDC security for R2E: token validation, JWKS caching and the
//! `AuthenticatedUser` identity.
//!
//! Enabled by default in the `r2e` facade (feature `security`) and reachable as
//! `r2e::r2e_security`.
//!
//! # Main types
//!
//! - [`AuthenticatedUser`] — the default identity, injected with
//!   `#[inject(identity)]` (struct field) or as a handler parameter. It
//!   implements `Identity` and [`RoleBasedIdentity`], which `#[roles(..)]` /
//!   `#[all_roles(..)]` require.
//! - [`JwtClaimsValidator`] / [`JwtValidator`] — signature and claim
//!   validation, against a [`JwksCache`] (production, OIDC providers) or a
//!   static key (tests, development). The extractor resolves an
//!   `Arc<JwtClaimsValidator>` bean **by type**, so it must be provided before
//!   `build_state()`.
//! - [`SecurityConfig`] — JWKS URL, issuer, audience, allowed algorithms.
//! - [`RoleExtractor`] / [`IdentityBuilder`] — customise where roles come from
//!   and which identity type a validated token produces.
//! - [`RolesGuard`] / [`AllRolesGuard`] — the guards behind `#[roles]` and
//!   `#[all_roles]`.
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use r2e::prelude::*;
//! use r2e::r2e_security::{AuthenticatedUser, JwksCache, JwtClaimsValidator, SecurityConfig};
//!
//! let config = SecurityConfig::new(
//!     "https://auth.example.com/.well-known/jwks.json",
//!     "https://auth.example.com",
//!     "my-api",
//! );
//! let jwks = Arc::new(JwksCache::new(config.clone()).await?);
//! let validator = JwtClaimsValidator::new(jwks, config);
//!
//! AppBuilder::new()
//!     .provide(Arc::new(validator))
//!     // ...
//!     .build_state().await;
//!
//! #[controller(path = "/me")]
//! pub struct MeController {
//!     #[inject(identity)] user: AuthenticatedUser,
//! }
//! ```
//!
//! # Guides
//!
//! [`docs/book/src/security/jwt-oidc.md`](https://github.com/plawn/r2e/blob/master/docs/book/src/security/jwt-oidc.md)
//! and the crate README. With feature `grpc`, the same validator authenticates
//! gRPC calls.

pub mod config;
pub mod error;
pub mod extractor;
#[cfg(feature = "grpc")]
pub mod grpc;
pub mod guards;
pub mod identity;
pub mod jwks;
pub mod jwt;
pub mod keycloak;
pub mod openid;

// Re-export primary public types for convenience.
pub use config::SecurityConfig;
pub use error::{SecurityError, SecurityErrorKind};
pub use extractor::{extract_jwt_claims, extract_jwt_claims_as, extract_jwt_identity};
pub use guards::{AllRolesGuard, RoleBasedIdentity, RolesDenied, RolesGuard};
pub use identity::{
    AuthenticatedUser, ClaimsIdentity, DefaultIdentityBuilder, DefaultRoleExtractor,
    FromValidatedJwtClaims, IdentityBuilder, IdentityBuilderWith,
};
/// The JWKS HTTP client builder doubles as the OAuth-metadata HTTP client
/// (same timeouts, same HTTPS-only policy) for crates building on
/// r2e-security, e.g. r2e-mcp discovery.
pub use jwks::build_jwks_client as build_oauth_http_client;
pub use jwks::JwksCache;
pub use jwt::{JwtClaimSet, JwtClaimsValidator, JwtValidator};

/// JWT signature algorithms accepted by [`SecurityConfig::with_allowed_algorithms`].
///
/// Re-exported so crates configuring validation (r2e-mcp's `mcp.auth.allowed-algorithms`)
/// can parse algorithm names (`Algorithm: FromStr`) without depending on `jsonwebtoken`.
pub use jsonwebtoken::Algorithm;
// The claim set carried by `AuthenticatedUser` / `Identity::claims()`. Defined
// in `r2e-core` (the `Identity` trait names it), re-exported here because every
// `r2e-security` signature that mentions claims uses it.
pub use r2e_core::{Audience, ClientAccess, RealmAccess, StandardClaims};

// Re-export the base RoleExtractor trait at crate root for convenience.
pub use openid::RoleExtractor;

// Re-export types needed by declarative macros and proc-macro generated code.
// This allows impl_claims_identity_extractor! to use $crate:: paths,
// and proc-macros to reference RolesGuard via r2e_security::__macro_support.
#[doc(hidden)]
pub mod __macro_support {
    pub use crate::guards::AllRolesGuard;
    pub use crate::guards::RolesGuard;
    pub use r2e_core::http;
    pub use r2e_core::type_list::HasBean;
    pub use r2e_core::web::extract::{FromRequestPartsVia, OptionalFromRequestPartsVia, ViaBean};
    pub use r2e_core::HttpError;
    pub use r2e_core::Identity;
    pub use r2e_core::Rejection;
    pub use r2e_core::StandardClaims;
    pub use serde_json;
}

pub mod prelude {
    //! Re-exports of the most commonly used security types.
    pub use crate::{
        AllRolesGuard, AuthenticatedUser, JwtValidator, RoleBasedIdentity, RolesGuard,
        SecurityConfig,
    };
}
