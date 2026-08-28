use std::convert::Infallible;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::{HeaderValue, Method, Request, Response};
use pin_project_lite::pin_project;
use tower::Service;

/// A multiplexing service that routes requests to a gRPC, a grpc-web, or an
/// HTTP service based on the `content-type` header.
///
/// - `content-type: application/grpc` (optionally `+proto`, `+json`, … or
///   `; charset=…`) → the gRPC service (`tonic::service::Routes`);
/// - `content-type: application/grpc-web*` (`+proto`, `-text`, …) → the
///   grpc-web arm. By default that arm is [`NoGrpcWeb`], which answers
///   `415 Unsupported Media Type`; [`with_grpc_web`](Self::with_grpc_web)
///   swaps in a real translation service (a `tonic-web` layered copy of the
///   routes — see `GrpcServer::with_grpc_web`, feature `web`);
/// - everything else → the HTTP (Axum) service.
///
/// When a grpc-web arm is installed, browser CORS preflights aimed at it
/// (`OPTIONS` carrying `x-grpc-web` in `access-control-request-headers`) are
/// routed to that arm too, so its CORS layer can answer them even when the
/// app has no `Cors` plugin.
///
/// All inner services must be infallible (`Error = Infallible`) — which
/// `tonic::service::Routes`, `r2e_core::http::Router` and a `tower-http`
/// `Cors`-wrapped `tonic_web::GrpcWebService` all are — so the multiplexer is
/// itself infallible and can be mounted directly on an axum router (the
/// `GrpcServer::multiplexed()` transport mounts it as the app's fallback
/// service).
#[derive(Clone)]
pub struct MultiplexService<GrpcSvc, HttpSvc, WebSvc = NoGrpcWeb> {
    grpc: GrpcSvc,
    http: HttpSvc,
    grpc_web: WebSvc,
    /// Whether grpc-web CORS preflights are routed to the grpc-web arm
    /// (only when a real arm is installed — `NoGrpcWeb` leaves them to the
    /// HTTP router, as before).
    preflight_to_web: bool,
}

impl<GrpcSvc, HttpSvc> MultiplexService<GrpcSvc, HttpSvc> {
    /// Create a new multiplexing service without grpc-web support
    /// (`application/grpc-web*` requests get a `415`).
    pub fn new(grpc: GrpcSvc, http: HttpSvc) -> Self {
        Self {
            grpc,
            http,
            grpc_web: NoGrpcWeb,
            preflight_to_web: false,
        }
    }
}

impl<GrpcSvc, HttpSvc, WebSvc> MultiplexService<GrpcSvc, HttpSvc, WebSvc> {
    /// Install a grpc-web arm: the service that receives every
    /// `application/grpc-web*` request (and grpc-web CORS preflights).
    ///
    /// `GrpcServer::with_grpc_web` builds it as
    /// `Cors<GrpcWebService<Routes>>`; anything infallible works.
    pub fn with_grpc_web<W>(self, grpc_web: W) -> MultiplexService<GrpcSvc, HttpSvc, W> {
        MultiplexService {
            grpc: self.grpc,
            http: self.http,
            grpc_web,
            preflight_to_web: true,
        }
    }
}

impl<GrpcSvc, HttpSvc, WebSvc, ReqBody, GrpcResBody, HttpResBody, WebResBody>
    Service<Request<ReqBody>> for MultiplexService<GrpcSvc, HttpSvc, WebSvc>
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
    WebSvc: Service<Request<ReqBody>, Response = Response<WebResBody>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    WebSvc::Future: Send + 'static,
    ReqBody: Send + 'static,
    GrpcResBody: http_body::Body<Data = Bytes> + Send + 'static,
    GrpcResBody::Error: Into<Box<dyn std::error::Error + Send + Sync>> + Send,
    HttpResBody: http_body::Body<Data = Bytes> + Send + 'static,
    HttpResBody::Error: Into<Box<dyn std::error::Error + Send + Sync>> + Send,
    WebResBody: http_body::Body<Data = Bytes> + Send + 'static,
    WebResBody::Error: Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    type Response = Response<MultiplexBody<GrpcResBody, HttpResBody, WebResBody>>;
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
            GrpcContentType::GrpcWeb => self.call_web(req),
            GrpcContentType::Other if self.preflight_to_web && is_grpc_web_preflight(&req) => {
                self.call_web(req)
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

impl<GrpcSvc, HttpSvc, WebSvc> MultiplexService<GrpcSvc, HttpSvc, WebSvc> {
    fn call_web<ReqBody, GrpcResBody, HttpResBody, WebResBody>(
        &mut self,
        req: Request<ReqBody>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        Response<MultiplexBody<GrpcResBody, HttpResBody, WebResBody>>,
                        Infallible,
                    >,
                > + Send,
        >,
    >
    where
        WebSvc: Service<Request<ReqBody>, Response = Response<WebResBody>, Error = Infallible>
            + Clone
            + Send
            + 'static,
        WebSvc::Future: Send + 'static,
        ReqBody: Send + 'static,
        GrpcResBody: Send + 'static,
        HttpResBody: Send + 'static,
        WebResBody: Send + 'static,
    {
        let mut web = self.grpc_web.clone();
        Box::pin(async move {
            match web.call(req).await {
                Ok(resp) => Ok(resp.map(|body| MultiplexBody::GrpcWeb { inner: body })),
                Err(infallible) => match infallible {},
            }
        })
    }
}

/// A browser CORS preflight for a grpc-web call: `OPTIONS` with
/// `access-control-request-headers` naming `x-grpc-web` (grpc-web and
/// connect-web clients always send that header on the real request, so the
/// preflight lists it). Preflights carry no `content-type`, hence this
/// dedicated check.
fn is_grpc_web_preflight<B>(req: &Request<B>) -> bool {
    req.method() == Method::OPTIONS
        && req
            .headers()
            .get_all(http::header::ACCESS_CONTROL_REQUEST_HEADERS)
            .iter()
            .any(|v| {
                v.to_str().is_ok_and(|s| {
                    s.split(',')
                        .any(|h| h.trim().eq_ignore_ascii_case("x-grpc-web"))
                })
            })
}

/// Message used both for the boot warning and for the 415 response body when
/// no grpc-web arm is installed.
pub const GRPC_WEB_UNSUPPORTED: &str = "grpc-web is not supported by r2e-grpc multiplexed mode";

/// How the multiplexer classifies a request's `content-type`.
///
/// A plain prefix match on `application/grpc` would also capture
/// `application/grpc-web` and `application/grpc-web-text`, routing them to the
/// raw tonic services with no `tonic-web` translation layer — the client then
/// gets a response it cannot parse. grpc-web therefore gets its own arm:
/// either a `tonic-web`-layered service ([`MultiplexService::with_grpc_web`])
/// or the [`NoGrpcWeb`] `415` responder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrpcContentType {
    /// `application/grpc`, `application/grpc+proto`, `application/grpc; …`.
    Grpc,
    /// `application/grpc-web`, `application/grpc-web-text`, `…+proto`.
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

/// The default grpc-web arm: answers every request with
/// `415 Unsupported Media Type` (body: [`GRPC_WEB_UNSUPPORTED`]) and a
/// per-request warning, instead of letting the raw tonic services produce a
/// response a grpc-web client cannot parse.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoGrpcWeb;

impl<B> Service<Request<B>> for NoGrpcWeb {
    type Response = Response<RejectedBody>;
    type Error = Infallible;
    type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        tracing::warn!(uri = %req.uri(), "rejecting request: {}", GRPC_WEB_UNSUPPORTED);
        let body = Bytes::from_static(GRPC_WEB_UNSUPPORTED.as_bytes());
        let mut resp = Response::new(RejectedBody {
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
        std::future::ready(Ok(resp))
    }
}

/// A fixed, fully-buffered body produced by [`NoGrpcWeb`] — yielded once,
/// then end-of-stream.
#[derive(Debug)]
pub struct RejectedBody {
    data: Option<Bytes>,
}

impl http_body::Body for RejectedBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(
            self.get_mut()
                .data
                .take()
                .map(|b| Ok(http_body::Frame::data(b))),
        )
    }

    fn is_end_stream(&self) -> bool {
        self.data.is_none()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        http_body::SizeHint::with_exact(self.data.as_ref().map_or(0, |b| b.len() as u64))
    }
}

pin_project! {
    /// Response body type for the multiplexer.
    ///
    /// Wraps the gRPC, HTTP, or grpc-web arm's response body.
    #[project = MultiplexBodyProj]
    #[non_exhaustive]
    pub enum MultiplexBody<G, H, W = RejectedBody> {
        Grpc { #[pin] inner: G },
        Http { #[pin] inner: H },
        GrpcWeb { #[pin] inner: W },
    }
}

impl<G, H, W> http_body::Body for MultiplexBody<G, H, W>
where
    G: http_body::Body<Data = Bytes>,
    G::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    H: http_body::Body<Data = Bytes>,
    H::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    W: http_body::Body<Data = Bytes>,
    W::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
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
            MultiplexBodyProj::GrpcWeb { inner } => inner
                .poll_frame(cx)
                .map(|opt| opt.map(|res| res.map_err(Into::into))),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            MultiplexBody::Grpc { inner } => inner.is_end_stream(),
            MultiplexBody::Http { inner } => inner.is_end_stream(),
            MultiplexBody::GrpcWeb { inner } => inner.is_end_stream(),
        }
    }

    fn size_hint(&self) -> http_body::SizeHint {
        match self {
            MultiplexBody::Grpc { inner } => inner.size_hint(),
            MultiplexBody::Http { inner } => inner.size_hint(),
            MultiplexBody::GrpcWeb { inner } => inner.size_hint(),
        }
    }
}

/// grpc-web arm builders (feature `web`).
#[cfg(feature = "web")]
pub mod web {
    use http::{HeaderName, Method};
    use tower_http::cors::{AllowHeaders, Any, CorsLayer};

    /// The CORS policy `GrpcServer::with_grpc_web()` applies to the grpc-web
    /// arm when none is given: any origin, `POST`/`OPTIONS`, the request's
    /// own headers mirrored back, the gRPC status trailers exposed, and a
    /// 24h preflight cache — what `tonic-web` shipped as its built-in
    /// default before delegating CORS to `tower-http`.
    pub fn default_cors() -> CorsLayer {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods([Method::POST, Method::OPTIONS])
            .allow_headers(AllowHeaders::mirror_request())
            .expose_headers([
                HeaderName::from_static("grpc-status"),
                HeaderName::from_static("grpc-message"),
                HeaderName::from_static("grpc-status-details-bin"),
            ])
            .max_age(std::time::Duration::from_secs(24 * 60 * 60))
    }

    /// Wrap tonic routes into a grpc-web arm: `Cors<GrpcWebService<Routes>>`.
    pub fn grpc_web_arm(
        routes: tonic::service::Routes,
        cors: CorsLayer,
    ) -> tower_http::cors::Cors<tonic_web::GrpcWebService<tonic::service::Routes>> {
        use tower::Layer;
        cors.layer(tonic_web::GrpcWebLayer::new().layer(routes))
    }
}
