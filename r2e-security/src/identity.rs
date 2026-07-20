use r2e_core::Identity;
use serde::Serialize;

use crate::error::SecurityError;
use crate::keycloak;
use crate::openid::{Merge, RoleExtractor, StandardRoleExtractor};

/// Simplified identity construction from JWT claims + app state.
///
/// Implement this trait to create custom identity types with minimal boilerplate.
/// Combined with [`impl_claims_identity_extractor!`], this replaces the need to
/// manually implement `FromRequestParts` for custom identity types.
///
/// `C` defaults to [`serde_json::Value`]. Use an application struct implementing
/// [`JwtClaimSet`](crate::JwtClaimSet) to deserialize custom fields directly.
///
/// # Example
///
/// ```ignore
/// use r2e_security::{FromValidatedJwtClaims, AuthenticatedUser, impl_claims_identity_extractor};
/// use r2e_core::HttpError;
///
/// pub struct DbUser {
///     pub auth: AuthenticatedUser,
///     pub profile: UserProfile,
/// }
///
/// impl FromValidatedJwtClaims<Services> for DbUser {
///     async fn from_jwt_claims(claims: serde_json::Value, state: &Services) -> Result<Self, HttpError> {
///         let auth = AuthenticatedUser::from_claims(claims);
///         let profile = fetch_profile(auth.sub(), &state.pool).await?;
///         Ok(DbUser { auth, profile })
///     }
/// }
///
/// impl_claims_identity_extractor!(DbUser);
/// ```
pub trait FromValidatedJwtClaims<S, C = serde_json::Value>: Identity + Sized
where
    C: crate::JwtClaimSet,
{
    fn from_jwt_claims(
        claims: C,
        state: &S,
    ) -> impl std::future::Future<Output = Result<Self, crate::__macro_support::HttpError>> + Send;
}

/// Deprecated name for [`FromValidatedJwtClaims`].
#[deprecated(
    since = "0.1.0",
    note = "use `FromValidatedJwtClaims`; it makes the validated-claims construction contract explicit"
)]
pub use FromValidatedJwtClaims as ClaimsIdentity;

/// Generate [`FromRequestPartsVia`](r2e_core::extract::FromRequestPartsVia)
/// and [`OptionalFromRequestPartsVia`](r2e_core::extract::OptionalFromRequestPartsVia)
/// implementations for an identity type that implements
/// [`FromValidatedJwtClaims`].
///
/// This macro eliminates the boilerplate of manually implementing extraction.
/// It pulls the `Arc<JwtClaimsValidator>` bean from the application state (via
/// a `HasBean` bound whose index witness is parked in the `ViaBean` marker),
/// validates the JWT, then delegates to
/// `FromValidatedJwtClaims::from_jwt_claims` for custom construction.
///
/// The optional impl enables `Option<YourIdentity>` as a handler parameter:
/// returns `None` when no `Authorization` header is present, and errors on
/// invalid JWTs.
///
/// # Usage
///
/// ```ignore
/// impl_claims_identity_extractor!(DbUser);
/// ```
///
/// One invocation generates implementations for one concrete identity type.
/// Generic identity families that need additional `impl<T>` parameters are not
/// supported; wrap a concrete instantiation in a local newtype instead.
/// Custom typed claims can be selected explicitly:
///
/// ```ignore
/// impl_claims_identity_extractor!(DbUser, claims = DbUserClaims);
/// ```
#[macro_export]
macro_rules! impl_claims_identity_extractor {
    ($identity:ty $(,)?) => {
        $crate::impl_claims_identity_extractor!(
            @impl $identity,
            $crate::__macro_support::serde_json::Value
        );
    };

    ($identity:ty, claims = $claims:ty $(,)?) => {
        $crate::impl_claims_identity_extractor!(@impl $identity, $claims);
    };

    (@impl $identity:ty, $claims:ty) => {
        impl<S, I>
            $crate::__macro_support::FromRequestPartsVia<S, $crate::__macro_support::ViaBean<I>>
            for $identity
        where
            S: $crate::__macro_support::HasBean<std::sync::Arc<$crate::JwtClaimsValidator>, I>
                + Send
                + Sync,
            I: Send + Sync,
            $claims: $crate::JwtClaimSet,
            $identity: $crate::FromValidatedJwtClaims<S, $claims>,
        {
            type Rejection = $crate::__macro_support::HttpError;

            async fn from_request_parts_via(
                parts: &mut $crate::__macro_support::http::header::Parts,
                state: &S,
            ) -> Result<Self, Self::Rejection> {
                let claims = $crate::extract_jwt_claims_as::<S, I, $claims>(parts, state).await?;
                <$identity as $crate::FromValidatedJwtClaims<S, $claims>>::from_jwt_claims(
                    claims, state,
                )
                .await
            }
        }

        impl<S, I>
            $crate::__macro_support::OptionalFromRequestPartsVia<
                S,
                $crate::__macro_support::ViaBean<I>,
            > for $identity
        where
            S: $crate::__macro_support::HasBean<std::sync::Arc<$crate::JwtClaimsValidator>, I>
                + Send
                + Sync,
            I: Send + Sync,
            $claims: $crate::JwtClaimSet,
            $identity: $crate::FromValidatedJwtClaims<S, $claims>,
        {
            type Rejection = $crate::__macro_support::HttpError;

            async fn from_request_parts_via(
                parts: &mut $crate::__macro_support::http::header::Parts,
                state: &S,
            ) -> Result<Option<Self>, Self::Rejection> {
                if !parts
                    .headers
                    .contains_key($crate::__macro_support::http::header::AUTHORIZATION)
                {
                    return Ok(None);
                }

                let claims = $crate::extract_jwt_claims_as::<S, I, $claims>(parts, state).await?;
                let identity =
                    <$identity as $crate::FromValidatedJwtClaims<S, $claims>>::from_jwt_claims(
                        claims, state,
                    )
                    .await?;
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
pub type DefaultRoleExtractor = Merge<StandardRoleExtractor, keycloak::RealmRoleExtractor>;

/// Default identity builder with automatic support for standard OIDC and Keycloak tokens.
///
/// This is the recommended builder for most use cases. It merges roles from:
/// 1. Standard OIDC `roles` claim
/// 2. Keycloak `realm_access.roles`
///
/// Roles from both sources are combined and deduplicated.
/// For more control, use [`IdentityBuilderWith`] with a custom extractor.
pub type DefaultIdentityBuilder = IdentityBuilderWith<DefaultRoleExtractor>;

impl Default for DefaultIdentityBuilder {
    fn default() -> Self {
        Self::new(Merge(StandardRoleExtractor, keycloak::RealmRoleExtractor))
    }
}

/// Represents an authenticated user extracted from a validated JWT token.
///
/// Intentionally **not** `Deserialize`: this is a trusted identity, constructed
/// only from a cryptographically validated JWT (via [`AuthenticatedUser::from_claims`]
/// / the `FromRequestParts` extractor). Deriving `Deserialize` would allow it to be
/// used as a request-body extractor (`Json<AuthenticatedUser>`), letting a client
/// forge their own `sub`/`roles`/`claims` and bypass authentication.
#[derive(Clone, Debug, Serialize)]
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
        let extractor = Merge(StandardRoleExtractor, keycloak::RealmRoleExtractor);
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
///
/// `sub` falls back to an empty string only if the claims lack one. Tokens that
/// reach this function through [`JwtClaimsValidator::validate`](crate::JwtClaimsValidator::validate)
/// are guaranteed a non-empty `sub` (validation rejects tokens without one); the
/// fallback only applies when constructing directly from arbitrary claims.
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
