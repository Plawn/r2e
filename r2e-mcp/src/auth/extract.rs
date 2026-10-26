//! First-class `#[inject(identity)]` extractors for the `mcp.auth` principal.
//!
//! An MCP member's `#[inject(identity)]` parameter is extracted exactly like
//! an HTTP route's — through
//! [`FromRequestPartsVia`](r2e_core::web::extract::FromRequestPartsVia)
//! against the application state. The `mcp.auth` layer validates the bearer
//! token *before* dispatch and deposits the result in the request extensions;
//! the impl here reads it back, under the [`ViaMcpAuth`] marker:
//!
//! | identity parameter | yields |
//! |---|---|
//! | [`McpPrincipal`] | the caller (`principal.user`, an `Arc<AuthenticatedUser>` — one refcount bump, no copy of the claims) + granted scopes + token hash |
//! | `Option<McpPrincipal>` | `None` when no principal is on the request |
//!
//! `McpPrincipal` implements [`Identity`](r2e_core::Identity) and
//! [`RoleBasedIdentity`](r2e_security::RoleBasedIdentity) (delegating to its
//! `user`), so `#[roles]` and custom guards work on it unchanged.
//!
//! A required identity on a request without a principal (no `mcp.auth`
//! configured, or a member reached through a transport the layer does not
//! wrap) is a 401 rejection, surfaced to the client as
//! [`McpError::Unauthorized`](crate::McpError::Unauthorized).
//!
//! # `AuthenticatedUser` itself
//!
//! There is deliberately **no** `ViaMcpAuth` impl for [`AuthenticatedUser`](r2e_security::AuthenticatedUser):
//! it already extracts through its bean-backed impl (marker `ViaBean`,
//! requiring the `Arc<JwtClaimsValidator>` bean — the HTTP JWT path), and a
//! second route would make the marker ambiguous. Under `mcp.auth`, declare
//! the identity as `McpPrincipal` (the caller is `principal.user`); keep
//! `AuthenticatedUser` only when the app provides `JwtClaimsValidator`
//! (HTTP-style JWT validation in the extractor). Migration guide:
//! `docs/migration/mcp-identity.md`.

use r2e_core::http::Parts;
use r2e_core::web::extract::{FromRequestPartsVia, OptionalFromRequestPartsVia};
use r2e_core::{Rejection, RejectionKind};

use super::validator::McpPrincipal;

/// Extraction marker for the identity deposited by the `mcp.auth` layer
/// ([`McpPrincipal`]). Inferred — never named in user code.
pub struct ViaMcpAuth;

const NO_PRINCIPAL: &str =
    "no MCP principal on this request: is `mcp.auth` configured? (an `AuthenticatedUser` \
     identity needs the `JwtClaimsValidator` bean instead)";

fn no_principal() -> Rejection {
    Rejection::new(RejectionKind::Unauthenticated, NO_PRINCIPAL)
}

impl<S: Sync> FromRequestPartsVia<S, ViaMcpAuth> for McpPrincipal {
    type Rejection = Rejection;

    fn from_request_parts_via(
        parts: &mut Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let principal = parts.extensions.get::<McpPrincipal>().cloned();
        std::future::ready(principal.ok_or_else(no_principal))
    }
}

impl<S: Sync> OptionalFromRequestPartsVia<S, ViaMcpAuth> for McpPrincipal {
    type Rejection = Rejection;

    fn from_request_parts_via(
        parts: &mut Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Option<Self>, Self::Rejection>> + Send {
        std::future::ready(Ok(parts.extensions.get::<McpPrincipal>().cloned()))
    }
}

/// `#[roles]`/`#[guard]` work on an `McpPrincipal` identity: it is the
/// caller's `AuthenticatedUser`.
impl r2e_core::Identity for McpPrincipal {
    fn sub(&self) -> &str {
        r2e_core::Identity::sub(&*self.user)
    }

    fn email(&self) -> Option<&str> {
        r2e_core::Identity::email(&*self.user)
    }

    fn claims(&self) -> Option<&r2e_core::StandardClaims> {
        r2e_core::Identity::claims(&*self.user)
    }
}

impl r2e_security::RoleBasedIdentity for McpPrincipal {
    fn roles(&self) -> &[String] {
        r2e_security::RoleBasedIdentity::roles(&*self.user)
    }
}
