pub mod config;
pub mod error;
pub mod extractor;
pub mod identity;
pub mod jwks;
pub mod jwt;
pub mod keycloak;
pub mod openid;

// Re-export primary public types for convenience.
pub use config::SecurityConfig;
pub use error::SecurityError;
pub use extractor::{extract_jwt_claims, extract_jwt_identity};
pub use identity::{
    AuthenticatedUser, ClaimsIdentity, DefaultIdentityBuilder, DefaultRoleExtractor,
    IdentityBuilder, IdentityBuilderWith,
};
pub use jwks::JwksCache;
pub use jwt::{JwtClaimsValidator, JwtValidator};

// Re-export the base RoleExtractor trait at crate root for convenience.
pub use openid::RoleExtractor;

// Re-export quarlus_core types needed by declarative macros.
// This allows impl_claims_identity_extractor! to use $crate:: paths.
#[doc(hidden)]
pub mod __macro_support {
    pub use quarlus_core::http;
    pub use quarlus_core::AppError;
    pub use quarlus_core::Identity;
}

pub mod prelude {
    //! Re-exports of the most commonly used security types.
    pub use crate::{AuthenticatedUser, JwtValidator, SecurityConfig};
}
