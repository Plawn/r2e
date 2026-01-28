pub mod config;
pub mod error;
pub mod extractor;
pub mod identity;
pub mod jwks;
pub mod jwt;

// Re-export primary public types for convenience.
pub use config::SecurityConfig;
pub use error::SecurityError;
pub use extractor::extract_jwt_identity;
pub use identity::{
    AuthenticatedUser, DefaultIdentityBuilder, DefaultRoleExtractor, IdentityBuilder,
    RoleExtractor,
};
pub use jwks::JwksCache;
pub use jwt::JwtValidator;
