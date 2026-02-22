use serde::{Deserialize, Serialize};

use crate::error::SecurityError;
use crate::keycloak;
use crate::openid::{Composite, RoleExtractor, StandardRoleExtractor};

/// Simplified identity construction from JWT claims + app state.
///
/// Implement this trait to create custom identity types with minimal boilerplate.
/// Combined with [`impl_claims_identity_extractor!`], this replaces the need to
/// manually implement `FromRequestParts` for custom identity types.
///
/// # Example
///
/// ```ignore
/// use r2e_security::{ClaimsIdentity, AuthenticatedUser, impl_claims_identity_extractor};
/// use r2e_core::HttpError;
///
/// #[derive(Clone)]
/// pub struct DbUser {
///     pub auth: AuthenticatedUser,
///     pub profile: UserProfile,
/// }
///
/// impl ClaimsIdentity<Services> for DbUser {
///     async fn from_jwt_claims(claims: serde_json::Value, state: &Services) -> Result<Self, HttpError> {
///         let auth = AuthenticatedUser::from_claims(claims);
///         let profile = fetch_profile(auth.sub(), &state.pool).await?;
///         Ok(DbUser { auth, profile })
///     }
/// }
///
/// impl_claims_identity_extractor!(DbUser);
/// ```
pub trait ClaimsIdentity<S>: Sized + Clone + Send + Sync {
    fn from_jwt_claims(
        claims: serde_json::Value,
        state: &S,
    ) -> impl std::future::Future<Output = Result<Self, crate::__macro_support::HttpError>> + Send;
}

/// Generate `FromRequestParts` and `OptionalFromRequestParts` implementations
/// for an identity type that implements [`ClaimsIdentity`].
///
/// This macro eliminates the boilerplate of manually implementing these traits.
/// It extracts and validates the JWT using `JwtClaimsValidator`, then delegates to
/// `ClaimsIdentity::from_jwt_claims` for custom construction.
///
/// The `OptionalFromRequestParts` impl enables `Option<YourIdentity>` as a handler
/// parameter: returns `None` when no `Authorization` header is present, and errors
/// on invalid JWTs.
///
/// # Usage
///
/// ```ignore
/// impl_claims_identity_extractor!(DbUser);
/// ```
#[macro_export]
macro_rules! impl_claims_identity_extractor {
    ($identity:ty) => {
        impl<S> $crate::__macro_support::http::extract::FromRequestParts<S> for $identity
        where
            S: Send + Sync,
            Self: $crate::ClaimsIdentity<S>,
            std::sync::Arc<$crate::JwtClaimsValidator>: $crate::__macro_support::http::extract::FromRef<S>,
        {
            type Rejection = $crate::__macro_support::HttpError;

            async fn from_request_parts(
                parts: &mut $crate::__macro_support::http::header::Parts,
                state: &S,
            ) -> Result<Self, Self::Rejection> {
                let claims = $crate::extract_jwt_claims(parts, state).await?;
                <$identity as $crate::ClaimsIdentity<S>>::from_jwt_claims(claims, state).await
            }
        }

        impl<S> $crate::__macro_support::http::extract::OptionalFromRequestParts<S> for $identity
        where
            S: Send + Sync,
            Self: $crate::ClaimsIdentity<S>,
            std::sync::Arc<$crate::JwtClaimsValidator>: $crate::__macro_support::http::extract::FromRef<S>,
        {
            type Rejection = $crate::__macro_support::HttpError;

            async fn from_request_parts(
                parts: &mut $crate::__macro_support::http::header::Parts,
                state: &S,
            ) -> Result<Option<Self>, Self::Rejection> {
                if !parts.headers.contains_key($crate::__macro_support::http::header::AUTHORIZATION) {
                    return Ok(None);
                }

                let claims = $crate::extract_jwt_claims(parts, state).await?;
                let identity = <$identity as $crate::ClaimsIdentity<S>>::from_jwt_claims(claims, state).await?;
                Ok(Some(identity))
            }
        }
    };
}

/// Trait for building an identity from validated JWT claims.
///
/// Implement this trait to customize how JWT claims are mapped to your
/// identity type. The `build` method is async, allowing database lookups
/// or other I/O during identity construction.
///
/// The default implementation ([`DefaultIdentityBuilder`]) produces
/// [`AuthenticatedUser`] synchronously from the claims.
///
/// # Example — sync (pure claims mapping)
///
/// ```ignore
/// struct MyIdentityBuilder;
///
/// impl IdentityBuilder for MyIdentityBuilder {
///     type Identity = MyUser;
///     fn build(&self, claims: serde_json::Value)
///         -> impl Future<Output = Result<MyUser, SecurityError>> + Send
///     {
///         let sub = claims.get("sub").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
///         let tenant = claims.get("tenant_id").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
///         std::future::ready(Ok(MyUser { sub, tenant_id: tenant }))
///     }
/// }
/// ```
///
/// # Example — async (database lookup)
///
/// ```ignore
/// struct DbIdentityBuilder { pool: SqlitePool }
///
/// impl IdentityBuilder for DbIdentityBuilder {
///     type Identity = DbUser;
///     fn build(&self, claims: serde_json::Value)
///         -> impl Future<Output = Result<DbUser, SecurityError>> + Send
///     {
///         let pool = self.pool.clone();
///         async move {
///             let sub = claims.get("sub").and_then(|v| v.as_str()).unwrap_or_default();
///             sqlx::query_as("SELECT * FROM users WHERE sub = ?")
///                 .bind(sub)
///                 .fetch_one(&pool)
///                 .await
///                 .map_err(|e| SecurityError::ValidationFailed(e.to_string()))
///         }
///     }
/// }
/// ```
pub trait IdentityBuilder: Send + Sync {
    type Identity: Clone + Send + Sync;
    fn build(
        &self,
        claims: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<Self::Identity, SecurityError>> + Send;
}

/// Identity builder that produces [`AuthenticatedUser`] using a configurable role extractor.
///
/// The type parameter `R` determines how roles are extracted from JWT claims.
/// Use [`DefaultIdentityBuilder`] for the common case with automatic Keycloak support.
///
/// # Example
///
/// ```ignore
/// use r2e_security::{IdentityBuilderWith, keycloak};
///
/// // Use Keycloak-specific extractor
/// let extractor = keycloak::RoleExtractor::new()
///     .with_realm_roles()
///     .with_client("my-api");
///
/// let builder = IdentityBuilderWith::new(extractor);
/// ```
#[derive(Debug)]
pub struct IdentityBuilderWith<R> {
    role_extractor: R,
}

impl<R> IdentityBuilderWith<R> {
    /// Create a new identity builder with the given role extractor.
    pub fn new(role_extractor: R) -> Self {
        Self { role_extractor }
    }

    /// Returns a reference to the role extractor.
    pub fn role_extractor(&self) -> &R {
        &self.role_extractor
    }
}

impl<R: RoleExtractor> IdentityBuilder for IdentityBuilderWith<R> {
    type Identity = AuthenticatedUser;

    fn build(
        &self,
        claims: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<AuthenticatedUser, SecurityError>> + Send {
        let user = build_authenticated_user(claims, &self.role_extractor);
        std::future::ready(Ok(user))
    }
}

/// Default role extractor: tries standard OIDC `roles` claim, then Keycloak `realm_access.roles`.
pub type DefaultRoleExtractor = Composite<StandardRoleExtractor, keycloak::RealmRoleExtractor>;

/// Default identity builder with automatic support for standard OIDC and Keycloak tokens.
///
/// This is the recommended builder for most use cases. It tries:
/// 1. Standard OIDC `roles` claim
/// 2. Keycloak `realm_access.roles`
///
/// For more control, use [`IdentityBuilderWith`] with a custom extractor.
pub type DefaultIdentityBuilder = IdentityBuilderWith<DefaultRoleExtractor>;

impl Default for DefaultIdentityBuilder {
    fn default() -> Self {
        Self::new(Composite(
            StandardRoleExtractor,
            keycloak::RealmRoleExtractor,
        ))
    }
}

/// Represents an authenticated user extracted from a validated JWT token.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthenticatedUser {
    /// Subject claim ("sub") - unique user identifier.
    pub sub: String,

    /// Email claim ("email"), if present in the token.
    pub email: Option<String>,

    /// Roles extracted from the token claims.
    pub roles: Vec<String>,

    /// Raw claims for advanced access.
    pub claims: serde_json::Value,
}

impl crate::__macro_support::Identity for AuthenticatedUser {
    fn sub(&self) -> &str {
        &self.sub
    }
    fn email(&self) -> Option<&str> {
        self.email.as_deref()
    }
    fn claims(&self) -> Option<&serde_json::Value> {
        Some(&self.claims)
    }
}

impl crate::guards::RoleBasedIdentity for AuthenticatedUser {
    fn roles(&self) -> &[String] {
        &self.roles
    }
}

impl AuthenticatedUser {
    /// Build an `AuthenticatedUser` from validated JWT claims.
    ///
    /// Uses the default role extractor (standard OIDC + Keycloak realm).
    /// For custom role extraction, use [`build_authenticated_user`] instead.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let claims = validator.validate_claims(token).await?;
    /// let user = AuthenticatedUser::from_claims(claims);
    /// ```
    pub fn from_claims(claims: serde_json::Value) -> Self {
        let extractor = Composite(StandardRoleExtractor, keycloak::RealmRoleExtractor);
        build_authenticated_user(claims, &extractor)
    }

    /// Build an `AuthenticatedUser` from claims with a custom role extractor.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let extractor = keycloak::RoleExtractor::new()
    ///     .with_realm_roles()
    ///     .with_client("my-api");
    ///
    /// let user = AuthenticatedUser::from_claims_with(claims, &extractor);
    /// ```
    pub fn from_claims_with(claims: serde_json::Value, extractor: &impl RoleExtractor) -> Self {
        build_authenticated_user(claims, extractor)
    }

    /// Check whether the user has a specific role.
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    /// Check whether the user has any of the specified roles.
    pub fn has_any_role(&self, roles: &[&str]) -> bool {
        roles.iter().any(|role| self.has_role(role))
    }
}

/// Build an `AuthenticatedUser` from validated JWT claims using the given role extractor.
pub fn build_authenticated_user(
    claims: serde_json::Value,
    role_extractor: &impl RoleExtractor,
) -> AuthenticatedUser {
    let sub = claims
        .get("sub")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let email = claims
        .get("email")
        .and_then(|v| v.as_str())
        .map(String::from);

    let roles = role_extractor.extract_roles(&claims);

    AuthenticatedUser {
        sub,
        email,
        roles,
        claims,
    }
}
