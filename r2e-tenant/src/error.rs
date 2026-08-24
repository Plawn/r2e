//! Tenancy failures and their HTTP mapping.
//!
//! Every way per-tenant routing can fail has exactly one status, chosen so the
//! caller can tell "you asked wrong" (4xx) from "we could not serve you"
//! (5xx) — and so a load balancer or a retrying client behaves sensibly:
//!
//! | Failure | Default status | Meaning |
//! |---|---|---|
//! | [`Unresolved`](TenantError::Unresolved) | 400 | no tenant in the request |
//! | [`Unknown`](TenantError::Unknown) | 404 | tenant not provisioned |
//! | [`Unavailable`](TenantError::Unavailable) | 503 | provisioned, but its resource could not be built (retryable) |
//! | [`Timeout`](TenantError::Timeout) | 504 | creating the resource took too long |
//! | [`Cycle`](TenantError::Cycle) | 500 | per-tenant resources depend on each other in a loop (a bug) |
//! | [`NoSource`](TenantError::NoSource) | 500 | the `PerTenant` plugin for that type was never installed (a bug) |
//!
//! The three request-driven statuses are configurable
//! ([`TenancyConfig`](crate::TenancyConfig): `missing-status`,
//! `unknown-status`, `unavailable-status`) because their right value depends on
//! the deployment — a gateway that maps the tenant itself may prefer 401/403
//! for a missing tenant, and a "silently empty" edge may prefer 404 over 400.
//! The three bug statuses are not configurable: a 500 is the correct answer.

use std::error::Error as StdError;
use std::fmt;
use std::sync::Arc;

use r2e_core::http::response::{IntoHttpResponse, IntoResponse, Response};
use r2e_core::http::StatusCode;
use r2e_core::HttpError;

use crate::TenantId;

/// A boxed cause, as returned by [`TenantSource::create`](crate::TenantSource::create).
pub type BoxError = Box<dyn StdError + Send + Sync>;

/// Why a per-tenant resource could not be handed to the caller.
#[derive(Clone)]
pub enum TenantError {
    /// No tenant could be resolved from the request.
    Unresolved,
    /// The tenant is not provisioned — the source returned `Ok(None)`.
    Unknown(TenantId),
    /// The tenant exists but its resource could not be created.
    Unavailable {
        /// The tenant whose resource failed.
        tenant: TenantId,
        /// The underlying cause, shared so the error stays `Clone`.
        source: Arc<dyn StdError + Send + Sync>,
    },
    /// Creating the resource exceeded `create-timeout`.
    Timeout(TenantId),
    /// Per-tenant resources form a dependency cycle. Carries the chain, most
    /// recent last: `A -> B -> A`.
    Cycle(String),
    /// No `PerTenant` plugin provides a map for this resource type (the
    /// cascade asked for a type that is not per-tenant).
    NoSource(&'static str),
}

impl TenantError {
    /// Wrap a source failure.
    pub fn unavailable(tenant: TenantId, source: BoxError) -> Self {
        Self::Unavailable {
            tenant,
            source: Arc::from(source),
        }
    }

    /// The tenant this failure is about, when it is about one.
    #[must_use]
    pub fn tenant(&self) -> Option<&TenantId> {
        match self {
            Self::Unknown(t) | Self::Timeout(t) | Self::Unavailable { tenant: t, .. } => Some(t),
            Self::Unresolved | Self::Cycle(_) | Self::NoSource(_) => None,
        }
    }

    /// Whether the failure is a framework/wiring bug rather than a bad request
    /// or a downstream outage (the 500 rows of the table in the module docs).
    #[must_use]
    pub fn is_bug(&self) -> bool {
        matches!(self, Self::Cycle(_) | Self::NoSource(_))
    }

    /// Map to an [`HttpError`] with the deployment's configured statuses.
    #[must_use]
    pub fn into_http_error(self, statuses: TenantStatuses) -> HttpError {
        let message = self.to_string();
        match self {
            Self::Unresolved => HttpError::from_status(statuses.missing, message),
            Self::Unknown(_) => HttpError::from_status(statuses.unknown, message),
            Self::Unavailable { .. } => HttpError::from_status(statuses.unavailable, message),
            Self::Timeout(_) => HttpError::from_status(StatusCode::GATEWAY_TIMEOUT, message),
            Self::Cycle(_) | Self::NoSource(_) => HttpError::internal(message),
        }
    }
}

impl fmt::Display for TenantError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unresolved => f.write_str("no tenant in request"),
            Self::Unknown(tenant) => write!(f, "unknown tenant `{tenant}`"),
            Self::Unavailable { tenant, source } => {
                write!(f, "tenant `{tenant}` is unavailable: {source}")
            }
            Self::Timeout(tenant) => {
                write!(f, "timed out creating the resource for tenant `{tenant}`")
            }
            Self::Cycle(chain) => write!(f, "per-tenant resource dependency cycle: {chain}"),
            Self::NoSource(ty) => write!(
                f,
                "no per-tenant source for `{ty}`: install the `PerTenant` plugin \
                 (`.plugin(PerTenant::<{ty}>::from::<MySource>())`)"
            ),
        }
    }
}

impl fmt::Debug for TenantError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TenantError({self})")
    }
}

impl StdError for TenantError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Unavailable { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl From<TenantError> for HttpError {
    fn from(err: TenantError) -> Self {
        err.into_http_error(TenantStatuses::default())
    }
}

impl IntoHttpResponse for TenantError {
    fn into_http_response(self) -> Response {
        HttpError::from(self).into_response()
    }
}

r2e_core::http::impl_into_response!(TenantError);

/// The three configurable tenancy statuses.
///
/// Carried by [`TenantRouter`](crate::TenantRouter) and
/// [`Tenanted<T>`](crate::Tenanted) so extractors map failures the way the
/// deployment asked for, without reading config per request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantStatuses {
    /// Status for [`TenantError::Unresolved`]. Default 400.
    pub missing: StatusCode,
    /// Status for [`TenantError::Unknown`]. Default 404.
    pub unknown: StatusCode,
    /// Status for [`TenantError::Unavailable`]. Default 503.
    pub unavailable: StatusCode,
}

impl Default for TenantStatuses {
    fn default() -> Self {
        Self {
            missing: StatusCode::BAD_REQUEST,
            unknown: StatusCode::NOT_FOUND,
            unavailable: StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}
