//! Borrowed view of the incoming request head.
//!
//! [`RequestHead`] is the shared vocabulary for framework machinery that runs
//! *before* the handler body and needs to look at the request without owning
//! it: guards (through [`GuardContext::head`](crate::GuardContext::head)) and
//! `#[managed]` resources (through
//! [`ManagedContext::request`](crate::ManagedContext::request)).
//!
//! It is a bundle of borrows — `Copy`, allocation-free, and valid only for the
//! duration of the acquire/check call it was handed to. Anything that must
//! outlive the call has to be cloned out of it.

use std::net::SocketAddr;

use crate::guards::PathParams;
use crate::http::{Extensions, HeaderMap, Method, Uri};

/// Read-only view of the request head available while acquiring a
/// `#[managed]` resource or running a guard.
///
/// Generated handlers build one per request (only when a route actually needs
/// it) from the extracted request parts, so every field is a plain borrow.
///
/// # Example
///
/// ```ignore
/// impl<S: BeanLookup + Send + Sync> ManagedResource<S> for TenantTx {
///     async fn acquire(context: ManagedContext<'_, S>) -> Result<Self, Self::Error> {
///         let head = context.require_request()?;
///         let tenant = head
///             .header("x-tenant-id")
///             .or_else(|| head.path_param("tenant"))
///             .ok_or_else(|| /* 400 */)?;
///         // ...
///     }
/// }
/// ```
#[derive(Clone, Copy)]
pub struct RequestHead<'a> {
    /// The request method.
    pub method: &'a Method,
    /// The request URI (path, query, and — for absolute-form requests — the
    /// authority).
    pub uri: &'a Uri,
    /// The request headers.
    pub headers: &'a HeaderMap,
    /// Request extensions, as populated by layers and extractors that ran
    /// before the handler (`ConnectInfo`, `MatchedPath`, anything a middleware
    /// or identity extractor parked there).
    pub extensions: &'a Extensions,
    /// Path parameters of the matched route pattern.
    pub path_params: PathParams<'a>,
    /// Transport peer address, when the server records connection info
    /// (`serve_auto` and the sharded server do). `None` under `TestApp`'s
    /// in-process dispatch.
    pub peer_addr: Option<SocketAddr>,
}

impl<'a> RequestHead<'a> {
    /// The request path.
    #[must_use]
    pub fn path(&self) -> &'a str {
        self.uri.path()
    }

    /// The request query string, if any.
    #[must_use]
    pub fn query_string(&self) -> Option<&'a str> {
        self.uri.query()
    }

    /// A header value as UTF-8, or `None` when it is absent or not UTF-8.
    ///
    /// Header names are case-insensitive; pass them lowercase.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&'a str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    /// A path parameter of the matched route pattern by name.
    #[must_use]
    pub fn path_param(&self, name: &str) -> Option<&str> {
        self.path_params.get(name)
    }

    /// The request host: the `Host` header when present, falling back to the
    /// URI authority (set for absolute-form request targets and HTTP/2
    /// `:authority`).
    #[must_use]
    pub fn host(&self) -> Option<&'a str> {
        self.header("host")
            .or_else(|| self.uri.authority().map(|authority| authority.as_str()))
    }

    /// A request extension by type — what a middleware or an identity
    /// extractor parked in `parts.extensions`.
    #[must_use]
    pub fn extension<T: Send + Sync + 'static>(&self) -> Option<&'a T> {
        self.extensions.get::<T>()
    }
}

impl std::fmt::Debug for RequestHead<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestHead")
            .field("method", &self.method)
            .field("uri", &self.uri)
            .field("peer_addr", &self.peer_addr)
            .finish_non_exhaustive()
    }
}
