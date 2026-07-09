use std::convert::Infallible;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::{HeaderValue, Request, Response};
use pin_project_lite::pin_project;
use tower::Service;

/// A multiplexing service that routes requests to either a gRPC or HTTP service
/// based on the `content-type` header.
///
/// Requests with `content-type: application/grpc*` are routed to the gRPC service,
/// all others to the HTTP (Axum) service.
///
/// Both inner services must be infallible (`Error = Infallible`) — which
/// `tonic::service::Routes` and `axum::Router` both are — so the multiplexer
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
    type Future = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let is_grpc = req
            .headers()
            .get(http::header::CONTENT_TYPE)
            .map(|ct| is_grpc_content_type(ct))
            .unwrap_or(false);

        if is_grpc {
            let mut grpc = self.grpc.clone();
            Box::pin(async move {
                match grpc.call(req).await {
                    Ok(resp) => Ok(resp.map(|body| MultiplexBody::Grpc { inner: body })),
                    Err(infallible) => match infallible {},
                }
            })
        } else {
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

/// Check if a content-type header value indicates a gRPC request.
///
/// The prefix match also captures `application/grpc-web*`, which is NOT
/// supported: grpc-web requests route to the plain tonic services (no
/// `tonic-web` translation layer) and will fail. If grpc-web support is ever
/// added, it needs its own branch with a `tonic-web` layer.
fn is_grpc_content_type(ct: &HeaderValue) -> bool {
    ct.as_bytes().starts_with(b"application/grpc")
}

pin_project! {
    /// Response body type for the multiplexer.
    ///
    /// Wraps either a gRPC or HTTP response body.
    #[project = MultiplexBodyProj]
    pub enum MultiplexBody<G, H> {
        Grpc { #[pin] inner: G },
        Http { #[pin] inner: H },
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
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            MultiplexBody::Grpc { inner } => inner.is_end_stream(),
            MultiplexBody::Http { inner } => inner.is_end_stream(),
        }
    }

    fn size_hint(&self) -> http_body::SizeHint {
        match self {
            MultiplexBody::Grpc { inner } => inner.size_hint(),
            MultiplexBody::Http { inner } => inner.size_hint(),
        }
    }
}
