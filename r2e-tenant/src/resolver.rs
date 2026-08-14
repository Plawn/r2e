//! SPI #1 — turning a request into a [`TenantId`].
//!
//! A resolver is a **bean**: register it (`.provide(...)` / `.register::<...>()`)
//! and name it on the plugin — `Tenancy::resolver::<MyResolver>()`. It runs once
//! per request, before any per-tenant resource is touched, and its answer is
//! memoized in `parts.extensions` so guards, extractors and `#[managed]`
//! resources all see the same tenant.
//!
//! Returning `Ok(None)` means "this request carries no tenant" — what happens
//! then is the deployment's call ([`MissingTenantPolicy`](crate::MissingTenantPolicy)),
//! not the resolver's. Returning `Err` is for a *malformed* tenant (a header
//! that is present but not a valid id): the resolver owns that status.
//!
//! # Implementing one
//!
//! Most resolvers do no I/O — implement [`SyncTenantResolver`] and get
//! [`TenantResolver`] for free:
//!
//! ```
//! use r2e_core::request_head::RequestHead;
//! use r2e_core::HttpError;
//! use r2e_tenant::{SyncTenantResolver, TenantId};
//!
//! #[derive(Clone)]
//! struct SubdomainResolver;
//!
//! impl SyncTenantResolver for SubdomainResolver {
//!     fn resolve_sync(&self, req: &RequestHead<'_>) -> Result<Option<TenantId>, HttpError> {
//!         let Some(host) = req.host() else { return Ok(None) };
//!         let Some((label, _)) = host.split_once('.') else { return Ok(None) };
//!         TenantId::parse(label)
//!             .map(Some)
//!             .map_err(|e| HttpError::bad_request(format!("invalid tenant subdomain: {e}")))
//!     }
//! }
//! ```
//!
//! # Tenants from a JWT
//!
//! There is deliberately **no JWT resolver here** — that would duplicate the
//! security layer's validation and put a second JWT parse on every request.
//! The pattern instead: the identity extractor (or a middleware) parks what it
//! already parsed in `parts.extensions`, and the resolver reads it back with
//! [`ExtensionTenantResolver`]:
//!
//! ```ignore
//! // in the identity extractor / a middleware:
//! parts.extensions.insert(TenantClaim(claims.tenant.clone()));
//!
//! // wiring:
//! .provide(ExtensionTenantResolver::<TenantClaim, _>::new(|c: &TenantClaim| {
//!     TenantId::parse(&c.0).ok()
//! }))
//! .plugin(Tenancy::resolver::<ExtensionTenantResolver<TenantClaim, _>>())
//! ```
//!
//! Extraction order makes this work: the request-data extractor (which holds the
//! identity) runs before the per-tenant extractors on the same route.

use std::borrow::Cow;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;

use r2e_core::request_head::RequestHead;
use r2e_core::HttpError;

use crate::TenantId;

/// A boxed, borrow-carrying future — the shape both tenancy SPIs return so they
/// stay object-safe (`Arc<dyn TenantResolver>`, `Arc<dyn TenantSource<T>>`).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// SPI #1: resolve the tenant of a request.
///
/// Implement [`SyncTenantResolver`] instead unless resolution needs `.await`
/// (a lookup in a tenant directory, a cache miss going to Redis).
pub trait TenantResolver: Send + Sync + 'static {
    /// Resolve the tenant, `Ok(None)` when the request carries none.
    fn resolve<'a>(
        &'a self,
        req: &'a RequestHead<'a>,
    ) -> BoxFuture<'a, Result<Option<TenantId>, HttpError>>;
}

/// Convenience form of [`TenantResolver`] for resolution that needs no `.await`.
///
/// A blanket impl bridges every `SyncTenantResolver` to `TenantResolver`, so a
/// type implements **one or the other**, never both.
pub trait SyncTenantResolver: Send + Sync + 'static {
    /// Resolve the tenant, `Ok(None)` when the request carries none.
    fn resolve_sync(&self, req: &RequestHead<'_>) -> Result<Option<TenantId>, HttpError>;
}

impl<T: SyncTenantResolver> TenantResolver for T {
    fn resolve<'a>(
        &'a self,
        req: &'a RequestHead<'a>,
    ) -> BoxFuture<'a, Result<Option<TenantId>, HttpError>> {
        let result = self.resolve_sync(req);
        Box::pin(std::future::ready(result))
    }
}

/// Reads the tenant from a request header (default `x-tenant-id`).
///
/// A present-but-invalid header is a `400`; an absent one is `Ok(None)`.
#[derive(Debug, Clone)]
pub struct HeaderTenantResolver {
    header: Cow<'static, str>,
}

impl HeaderTenantResolver {
    /// The default header name.
    pub const DEFAULT_HEADER: &'static str = "x-tenant-id";

    /// Read the tenant from `header` (compared lowercase, as HTTP requires).
    #[must_use]
    pub fn new(header: impl Into<Cow<'static, str>>) -> Self {
        let header = match header.into() {
            Cow::Borrowed(s) if s.bytes().all(|b| !b.is_ascii_uppercase()) => Cow::Borrowed(s),
            other => Cow::Owned(other.to_ascii_lowercase()),
        };
        Self { header }
    }

    /// The header this resolver reads.
    #[must_use]
    pub fn header(&self) -> &str {
        &self.header
    }
}

impl Default for HeaderTenantResolver {
    fn default() -> Self {
        Self::new(Self::DEFAULT_HEADER)
    }
}

impl SyncTenantResolver for HeaderTenantResolver {
    fn resolve_sync(&self, req: &RequestHead<'_>) -> Result<Option<TenantId>, HttpError> {
        match req.header(&self.header) {
            None => Ok(None),
            Some(raw) => TenantId::parse(raw).map(Some).map_err(|err| {
                HttpError::bad_request(format!("invalid `{}` header: {err}", self.header))
            }),
        }
    }
}

/// Reads the tenant from a path parameter of the matched route
/// (`/t/{tenant}/...`).
#[derive(Debug, Clone)]
pub struct PathTenantResolver {
    param: Cow<'static, str>,
}

impl PathTenantResolver {
    /// The default path-parameter name.
    pub const DEFAULT_PARAM: &'static str = "tenant";

    /// Read the tenant from the `param` path parameter.
    #[must_use]
    pub fn new(param: impl Into<Cow<'static, str>>) -> Self {
        Self {
            param: param.into(),
        }
    }

    /// The path parameter this resolver reads.
    #[must_use]
    pub fn param(&self) -> &str {
        &self.param
    }
}

impl Default for PathTenantResolver {
    fn default() -> Self {
        Self::new(Self::DEFAULT_PARAM)
    }
}

impl SyncTenantResolver for PathTenantResolver {
    fn resolve_sync(&self, req: &RequestHead<'_>) -> Result<Option<TenantId>, HttpError> {
        match req.path_param(&self.param) {
            None => Ok(None),
            Some(raw) => TenantId::parse(raw).map(Some).map_err(|err| {
                HttpError::bad_request(format!("invalid `{{{}}}` path parameter: {err}", self.param))
            }),
        }
    }
}

/// Projects a request extension into a tenant id.
///
/// The bridge for tenants that come from something already parsed upstream —
/// JWT claims, a session, a gateway-injected struct. See the module docs.
pub struct ExtensionTenantResolver<T, F> {
    project: F,
    _extension: PhantomData<fn() -> T>,
}

impl<T, F> ExtensionTenantResolver<T, F>
where
    T: Send + Sync + 'static,
    F: Fn(&T) -> Option<TenantId> + Send + Sync + 'static,
{
    /// Read extension `T` and project it to a tenant id.
    pub fn new(project: F) -> Self {
        Self {
            project,
            _extension: PhantomData,
        }
    }
}

impl<T, F: Clone> Clone for ExtensionTenantResolver<T, F> {
    fn clone(&self) -> Self {
        Self {
            project: self.project.clone(),
            _extension: PhantomData,
        }
    }
}

impl<T, F> SyncTenantResolver for ExtensionTenantResolver<T, F>
where
    T: Send + Sync + 'static,
    F: Fn(&T) -> Option<TenantId> + Send + Sync + 'static,
{
    fn resolve_sync(&self, req: &RequestHead<'_>) -> Result<Option<TenantId>, HttpError> {
        Ok(req.extension::<T>().and_then(|ext| (self.project)(ext)))
    }
}

/// A resolver from a plain closure — for one-off wiring and tests.
///
/// ```
/// use r2e_tenant::{FnTenantResolver, TenantId};
///
/// let resolver = FnTenantResolver::new(|req: &r2e_core::request_head::RequestHead<'_>| {
///     Ok(req.header("x-org").and_then(|raw| TenantId::parse(raw).ok()))
/// });
/// ```
pub struct FnTenantResolver<F> {
    resolve: F,
}

impl<F> FnTenantResolver<F>
where
    F: Fn(&RequestHead<'_>) -> Result<Option<TenantId>, HttpError> + Send + Sync + 'static,
{
    /// Wrap `resolve` as a [`TenantResolver`].
    pub fn new(resolve: F) -> Self {
        Self { resolve }
    }
}

impl<F: Clone> Clone for FnTenantResolver<F> {
    fn clone(&self) -> Self {
        Self {
            resolve: self.resolve.clone(),
        }
    }
}

impl<F> SyncTenantResolver for FnTenantResolver<F>
where
    F: Fn(&RequestHead<'_>) -> Result<Option<TenantId>, HttpError> + Send + Sync + 'static,
{
    fn resolve_sync(&self, req: &RequestHead<'_>) -> Result<Option<TenantId>, HttpError> {
        (self.resolve)(req)
    }
}
