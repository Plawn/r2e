use std::convert::Infallible;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::{HeaderValue, Request, Response};
use pin_project_lite::pin_project;
use tower::Service;

/// A multiplexing service that routes requests to either a gRPC or HTTP service
/// based on the `content-type` header.
///
/// Requests with `content-type: application/grpc` (optionally `+proto`, `+json`,
/// … or `; charset=…`) are routed to the gRPC service, all others to the HTTP
/// (Axum) service. `application/grpc-web*` is **not supported** and is rejected
/// with `415 Unsupported Media Type` — see [`GrpcContentType`].
///
/// Both inner services must be infallible (`Error = Infallible`) — which
/// `tonic::service::Routes` and `r2e_core::http::Router` both are — so the multiplexer
/// is itself infallible and can be mounted directly on an axum router (the
/// `GrpcServer::multiplexed()` transport mounts it as the app's fallback
/// service).
#[derive(Clone)]
pub struct MultiplexService<GrpcSvc, HttpSvc> {
    grpc: GrpcSvc,
    http: HttpSvc,
}

impl<GrpcSvc, HttpSvc> MultiplexService<GrpcSvc, HttpSvc> {
    /// Create a new multiplexing service.
    pub fn new(grpc: GrpcSvc, http: HttpSvc) -> Self {
        Self { grpc, http }
    }
}

impl<GrpcSvc, HttpSvc, ReqBody, GrpcResBody, HttpResBody> Service<Request<ReqBody>>
    for MultiplexService<GrpcSvc, HttpSvc>
where
    GrpcSvc: Service<Request<ReqBody>, Response = Response<GrpcResBody>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    GrpcSvc::Future: Send + 'static,
    HttpSvc: Service<Request<ReqBody>, Response = Response<HttpResBody>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    HttpSvc::Future: Send + 'static,
    ReqBody: Send + 'static,
    GrpcResBody: http_body::Body<Data = Bytes> + Send + 'static,
    GrpcResBody::Error: Into<Box<dyn std::error::Error + Send + Sync>> + Send,
    HttpResBody: http_body::Body<Data = Bytes> + Send + 'static,
    HttpResBody::Error: Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    type Response = Response<MultiplexBody<GrpcResBody, HttpResBody>>;
    type Error = Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let kind = req
            .headers()
            .get(http::header::CONTENT_TYPE)
            .map(GrpcContentType::classify)
            .unwrap_or(GrpcContentType::Other);

        match kind {
            GrpcContentType::Grpc => {
                let mut grpc = self.grpc.clone();
                Box::pin(async move {
                    match grpc.call(req).await {
                        Ok(resp) => Ok(resp.map(|body| MultiplexBody::Grpc { inner: body })),
                        Err(infallible) => match infallible {},
                    }
                })
            }
            GrpcContentType::GrpcWeb => {
                // Forwarding this to tonic would produce a garbled response
                // (no `tonic-web` translation layer), so fail explicitly.
                tracing::warn!(uri = %req.uri(), "rejecting request: {}", GRPC_WEB_UNSUPPORTED);
                Box::pin(async move { Ok(grpc_web_unsupported_response()) })
            }
            GrpcContentType::Other => {
                let mut http = self.http.clone();
                Box::pin(async move {
                    match http.call(req).await {
                        Ok(resp) => Ok(resp.map(|body| MultiplexBody::Http { inner: body })),
                        Err(infallible) => match infallible {},
                    }
                })
            }
        }
    }
}

/// Message used both for the boot warning and for the 415 response body.
pub const GRPC_WEB_UNSUPPORTED: &str = "grpc-web is not supported by r2e-grpc multiplexed mode";

/// How the multiplexer classifies a request's `content-type`.
///
/// A plain prefix match on `application/grpc` would also capture
/// `application/grpc-web` and `application/grpc-web-text`, routing them to the
/// raw tonic services with no `tonic-web` translation layer — the client then
/// gets a response it cannot parse. grpc-web therefore gets its own arm and a
/// clear `415`. If grpc-web support is ever added, that arm is where a
/// `tonic-web`-layered service belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrpcContentType {
    /// `application/grpc`, `application/grpc+proto`, `application/grpc; …`.
    Grpc,
    /// `application/grpc-web`, `application/grpc-web-text`, `…+proto`. Unsupported.
    GrpcWeb,
    /// Anything else — handled by the HTTP router.
    Other,
}

impl GrpcContentType {
    /// Classify a `content-type` header value.
    pub fn classify(ct: &HeaderValue) -> Self {
        let Some(rest) = ct.as_bytes().strip_prefix(b"application/grpc") else {
            return Self::Other;
        };
        match rest.first() {
            // `application/grpc` exactly, or a parameter/subtype suffix.
            None | Some(b'+') | Some(b';') | Some(b' ') => Self::Grpc,
            // `application/grpc-web`, `application/grpc-web-text`, …
            Some(b'-') if rest.starts_with(b"-web") => Self::GrpcWeb,
            // `application/grpcfoo` is not gRPC at all.
            _ => Self::Other,
        }
    }
}

/// Build the `415 Unsupported Media Type` response for a grpc-web request.
fn grpc_web_unsupported_response<G, H>() -> Response<MultiplexBody<G, H>> {
    let body = Bytes::from_static(GRPC_WEB_UNSUPPORTED.as_bytes());
    let mut resp = Response::new(MultiplexBody::Rejected {
        data: Some(body.clone()),
    });
    *resp.status_mut() = http::StatusCode::UNSUPPORTED_MEDIA_TYPE;
    resp.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    resp.headers_mut().insert(
        http::header::CONTENT_LENGTH,
        HeaderValue::from(body.len() as u64),
    );
    resp
}

pin_project! {
    /// Response body type for the multiplexer.
    ///
    /// Wraps either a gRPC or HTTP response body.
    #[project = MultiplexBodyProj]
    pub enum MultiplexBody<G, H> {
        Grpc { #[pin] inner: G },
        Http { #[pin] inner: H },
        /// A fixed, fully-buffered body produced by the multiplexer itself
        /// (the grpc-web rejection) — yielded once, then end-of-stream.
        Rejected { data: Option<Bytes> },
    }
}

impl<G, H> http_body::Body for MultiplexBody<G, H>
where
    G: http_body::Body<Data = Bytes>,
    G::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    H: http_body::Body<Data = Bytes>,
    H::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Data = Bytes;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        match self.project() {
            MultiplexBodyProj::Grpc { inner } => inner
                .poll_frame(cx)
                .map(|opt| opt.map(|res| res.map_err(Into::into))),
            MultiplexBodyProj::Http { inner } => inner
                .poll_frame(cx)
                .map(|opt| opt.map(|res| res.map_err(Into::into))),
            MultiplexBodyProj::Rejected { data } => {
                Poll::Ready(data.take().map(|b| Ok(http_body::Frame::data(b))))
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            MultiplexBody::Grpc { inner } => inner.is_end_stream(),
            MultiplexBody::Http { inner } => inner.is_end_stream(),
            MultiplexBody::Rejected { data } => data.is_none(),
        }
    }

    fn size_hint(&self) -> http_body::SizeHint {
        match self {
            MultiplexBody::Grpc { inner } => inner.size_hint(),
            MultiplexBody::Http { inner } => inner.size_hint(),
            MultiplexBody::Rejected { data } => {
                http_body::SizeHint::with_exact(data.as_ref().map_or(0, |b| b.len() as u64))
            }
        }
    }
}
