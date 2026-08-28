//! `MultiplexService` content-type routing, including the grpc-web rejection.

use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::{HeaderValue, Request, Response, StatusCode};
use r2e_grpc::multiplex::{GrpcContentType, MultiplexService, GRPC_WEB_UNSUPPORTED};
use tower::Service;

// --- Test doubles -------------------------------------------------------

/// A one-shot body carrying a fixed payload.
struct StubBody(Option<Bytes>);

impl http_body::Body for StubBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        Poll::Ready(this.0.take().map(|b| Ok(http_body::Frame::data(b))))
    }
}

/// A service that answers every request with a fixed marker payload.
#[derive(Clone)]
struct MarkerService(&'static str);

impl<B: Send + 'static> Service<Request<B>> for MarkerService {
    type Response = Response<StubBody>;
    type Error = Infallible;
    type Future =
        Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: Request<B>) -> Self::Future {
        let marker = self.0;
        Box::pin(async move {
            Ok(Response::new(StubBody(Some(Bytes::from_static(
                marker.as_bytes(),
            )))))
        })
    }
}

fn mux() -> MultiplexService<MarkerService, MarkerService> {
    MultiplexService::new(MarkerService("grpc"), MarkerService("http"))
}

fn request(content_type: Option<&str>) -> Request<StubBody> {
    let mut req = Request::new(StubBody(None));
    *req.uri_mut() = "/pkg.Service/Method".parse().unwrap();
    if let Some(ct) = content_type {
        req.headers_mut().insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_str(ct).unwrap(),
        );
    }
    req
}

/// Drain a body into `Bytes` without pulling in `http-body-util`.
async fn collect<B>(mut body: B) -> Bytes
where
    B: http_body::Body<Data = Bytes> + Unpin,
{
    let mut out = Vec::new();
    loop {
        let frame = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await;
        match frame {
            Some(Ok(f)) => {
                if let Ok(data) = f.into_data() {
                    out.extend_from_slice(&data);
                }
            }
            Some(Err(_)) => panic!("body error"),
            None => break,
        }
    }
    Bytes::from(out)
}

// --- content-type classification ---------------------------------------

fn classify(ct: &str) -> GrpcContentType {
    GrpcContentType::classify(&HeaderValue::from_str(ct).unwrap())
}

#[test]
fn classify_recognises_grpc() {
    assert_eq!(classify("application/grpc"), GrpcContentType::Grpc);
    assert_eq!(classify("application/grpc+proto"), GrpcContentType::Grpc);
    assert_eq!(classify("application/grpc+json"), GrpcContentType::Grpc);
    assert_eq!(
        classify("application/grpc; charset=utf-8"),
        GrpcContentType::Grpc
    );
}

#[test]
fn classify_separates_grpc_web() {
    assert_eq!(classify("application/grpc-web"), GrpcContentType::GrpcWeb);
    assert_eq!(
        classify("application/grpc-web+proto"),
        GrpcContentType::GrpcWeb
    );
    assert_eq!(
        classify("application/grpc-web-text"),
        GrpcContentType::GrpcWeb
    );
}

#[test]
fn classify_leaves_everything_else_to_http() {
    assert_eq!(classify("application/json"), GrpcContentType::Other);
    assert_eq!(classify("text/plain"), GrpcContentType::Other);
    // Not gRPC: an unrelated subtype that merely starts with the same bytes.
    assert_eq!(classify("application/grpcfoo"), GrpcContentType::Other);
}

// --- routing ------------------------------------------------------------

#[r2e_core::test]
async fn grpc_content_type_goes_to_the_grpc_service() {
    let resp = mux().call(request(Some("application/grpc"))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(&collect(resp.into_body()).await[..], b"grpc");
}

#[r2e_core::test]
async fn other_content_types_go_to_the_http_service() {
    let resp = mux().call(request(Some("application/json"))).await.unwrap();
    assert_eq!(&collect(resp.into_body()).await[..], b"http");

    // No content-type at all is an ordinary HTTP request too.
    let resp = mux().call(request(None)).await.unwrap();
    assert_eq!(&collect(resp.into_body()).await[..], b"http");
}

#[r2e_core::test]
async fn grpc_web_is_rejected_with_415_instead_of_reaching_tonic() {
    for ct in [
        "application/grpc-web",
        "application/grpc-web+proto",
        "application/grpc-web-text",
    ] {
        let resp = mux().call(request(Some(ct))).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "content-type: {ct}"
        );
        assert_eq!(
            resp.headers()
                .get(http::header::CONTENT_TYPE)
                .map(|v| v.to_str().unwrap()),
            Some("text/plain; charset=utf-8")
        );
        let body = collect(resp.into_body()).await;
        assert_eq!(body, Bytes::from_static(GRPC_WEB_UNSUPPORTED.as_bytes()));
    }
}
