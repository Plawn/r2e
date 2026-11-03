//! The per-call request head: a gRPC call seen as HTTP request [`Parts`].
//!
//! Every transport hands its members the same head — `http::request::Parts`
//! — so `#[inject(identity)]`, `#[inject(request)]`, struct-level identity
//! and every `FromRequestPartsVia` extractor run on gRPC exactly as on an
//! HTTP route, and guards read one [`GuardContext`](r2e_core::GuardContext)
//! built by [`GuardContext::from_parts`](r2e_core::GuardContext::from_parts).
//!
//! The generated `#[grpc_routes]` dispatch takes the incoming
//! [`tonic::Request`] apart with [`into_head`] before anything runs, and
//! puts it back together with [`from_head`] right before the user method:
//!
//! | `Parts` field | gRPC value |
//! |---|---|
//! | `method` | `POST` (every unary/streaming gRPC call is an HTTP/2 POST) |
//! | `uri` | the call's method path, `/<package.Service>/<Method>` (captured by [`CaptureMethodPath`], which `#[grpc_routes]` wraps every service in) |
//! | `version` | `HTTP/2.0` |
//! | `headers` | the request metadata, moved without a copy ([`MetadataMap::into_headers`]) — `headers.get("x-tenant-id")` reads a metadata key, and the `authorization` header an HTTP identity extractor reads is the gRPC `authorization` metadata |
//! | `extensions` | the request extensions (moved), plus [`ConnectInfo`] built from [`tonic::Request::remote_addr`] and — when the service extracts anything — a copy of the [`MetadataMap`] for extractors that want the tonic view |
//!
//! Extractors may mutate the head (insert extensions, …): [`from_head`]
//! rebuilds the tonic request from the head as the extractors left it.

use std::net::SocketAddr;
use std::task::{Context, Poll};

use r2e_core::http::{ConnectInfo, Method, Parts, Uri};
use tonic::metadata::MetadataMap;

/// The method path (`/<package.Service>/<Method>`) of a gRPC call, deposited
/// in the request extensions by [`CaptureMethodPath`] and moved into the
/// head's `uri` by [`into_head`].
#[derive(Clone, Debug)]
pub struct MethodPath(pub Uri);

/// Take a tonic request apart into its HTTP head and its message.
///
/// The metadata becomes the head's headers and the extensions move over —
/// no copy. `method_path` is called only when the request does not carry a
/// [`MethodPath`] (a call that did not come through [`CaptureMethodPath`],
/// e.g. a test invoking the service directly). With `deposit_metadata`, a
/// copy of the metadata is also deposited in the extensions as a
/// [`MetadataMap`] (one header-map clone; the generated code asks for it only
/// when the service runs an extractor).
pub fn into_head<T>(
    request: tonic::Request<T>,
    method_path: impl FnOnce() -> Uri,
    deposit_metadata: bool,
) -> (Parts, T) {
    let remote_addr: Option<SocketAddr> = request.remote_addr();
    let (metadata, extensions, message) = request.into_parts();
    let (mut parts, ()) = http::Request::new(()).into_parts();
    parts.method = Method::POST;
    parts.version = http::Version::HTTP_2;
    parts.extensions = extensions;
    parts.uri = match parts.extensions.remove::<MethodPath>() {
        Some(MethodPath(uri)) => uri,
        None => method_path(),
    };
    if let Some(addr) = remote_addr {
        parts.extensions.insert(ConnectInfo(addr));
    }
    if deposit_metadata {
        parts.extensions.insert(metadata.clone());
    }
    parts.headers = metadata.into_headers();
    (parts, message)
}

/// Rebuild the tonic request from a head (as the extractors left it) and the
/// message taken apart by [`into_head`].
///
/// The headers are the source of truth for the metadata; the deposited
/// [`MetadataMap`] copy, if any, is dropped so the user method never sees a
/// stale duplicate.
pub fn from_head<T>(mut parts: Parts, message: T) -> tonic::Request<T> {
    parts.extensions.remove::<MetadataMap>();
    tonic::Request::from_parts(
        MetadataMap::from_headers(parts.headers),
        parts.extensions,
        message,
    )
}

/// The fallback method path of a call that carries no [`MethodPath`]:
/// `/<service>/<method>`.
pub fn fallback_method_path(service: &str, method: &str) -> Uri {
    Uri::try_from(format!("/{service}/{method}")).unwrap_or_else(|_| Uri::from_static("/"))
}

/// A transparent service wrapper depositing the request's method path in its
/// extensions as a [`MethodPath`] — tonic's server hands the generated trait
/// methods a request without its URI, and [`into_head`] puts it back.
///
/// `#[grpc_routes]` wraps every registered service in it; the cost is one
/// `Uri` clone per call (reference-counted bytes).
#[derive(Clone, Debug)]
pub struct CaptureMethodPath<S>(pub S);

impl<S: tonic::server::NamedService> tonic::server::NamedService for CaptureMethodPath<S> {
    const NAME: &'static str = S::NAME;
}

impl<S, B> tower::Service<http::Request<B>> for CaptureMethodPath<S>
where
    S: tower::Service<http::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, mut request: http::Request<B>) -> Self::Future {
        let uri = request.uri().clone();
        request.extensions_mut().insert(MethodPath(uri));
        self.0.call(request)
    }
}
