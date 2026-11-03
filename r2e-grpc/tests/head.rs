//! The per-call head (`r2e_grpc::head`): a tonic request taken apart into
//! HTTP `Parts` and put back together, and the method-path capture layer.

use std::convert::Infallible;
use std::net::SocketAddr;

use r2e_core::http::{ConnectInfo, Method, Uri};
use r2e_grpc::{from_head, into_head, CaptureMethodPath, MethodPath};
use tonic::metadata::MetadataMap;
use tonic::Request;

fn fallback() -> Uri {
    r2e_grpc::__macro_support::fallback_method_path("greeter.Greeter", "SayHello")
}

fn request() -> Request<&'static str> {
    let mut request = Request::new("msg");
    request
        .metadata_mut()
        .insert("authorization", "Bearer t0k".parse().unwrap());
    request.metadata_mut().insert("x-tenant-id", "acme".parse().unwrap());
    request
}

#[test]
fn head_maps_metadata_method_and_version() {
    let (parts, message) = into_head(request(), fallback, false);
    assert_eq!(message, "msg");
    assert_eq!(parts.method, Method::POST);
    assert_eq!(parts.version, http::Version::HTTP_2);
    assert_eq!(parts.uri.path(), "/greeter.Greeter/SayHello");
    assert_eq!(parts.headers.get("authorization").unwrap(), "Bearer t0k");
    assert_eq!(parts.headers.get("x-tenant-id").unwrap(), "acme");
    // No peer without a transport, and no metadata copy unless asked.
    assert!(parts.extensions.get::<ConnectInfo<SocketAddr>>().is_none());
    assert!(parts.extensions.get::<MetadataMap>().is_none());
}

#[test]
fn captured_method_path_wins_over_the_fallback() {
    let mut request = request();
    request
        .extensions_mut()
        .insert(MethodPath(Uri::from_static("/pkg.Svc/Real")));
    let (parts, _) = into_head(request, || panic!("fallback not needed"), false);
    assert_eq!(parts.uri.path(), "/pkg.Svc/Real");
    assert!(parts.extensions.get::<MethodPath>().is_none(), "consumed");
}

#[test]
fn metadata_is_deposited_on_request_and_dropped_on_rebuild() {
    let (mut parts, message) = into_head(request(), fallback, true);
    let deposited = parts.extensions.get::<MetadataMap>().unwrap();
    assert_eq!(deposited.get("x-tenant-id").unwrap(), "acme");

    // Extractors may edit the head; the rebuilt request sees their edits.
    parts.headers.insert("x-added", "1".parse().unwrap());
    parts.extensions.insert(7u32);
    let request = from_head(parts, message);
    assert_eq!(request.get_ref(), &"msg");
    assert_eq!(request.metadata().get("x-added").unwrap(), "1");
    assert_eq!(request.metadata().get("authorization").unwrap(), "Bearer t0k");
    assert_eq!(request.extensions().get::<u32>(), Some(&7));
    assert!(request.extensions().get::<MetadataMap>().is_none());
}

#[derive(Clone)]
struct Echo;

impl tower::Service<http::Request<()>> for Echo {
    type Response = Option<Uri>;
    type Error = Infallible;
    type Future = std::future::Ready<Result<Self::Response, Infallible>>;

    fn poll_ready(
        &mut self,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Infallible>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<()>) -> Self::Future {
        std::future::ready(Ok(request
            .extensions()
            .get::<MethodPath>()
            .map(|m| m.0.clone())))
    }
}

#[test]
fn capture_layer_deposits_the_request_uri() {
    use tower::Service;

    let mut svc = CaptureMethodPath(Echo);
    let request = http::Request::builder()
        .uri("/greeter.Greeter/SayHello")
        .body(())
        .unwrap();
    let fut = svc.call(request);
    let seen = futures_ready(fut);
    assert_eq!(seen.unwrap().path(), "/greeter.Greeter/SayHello");
}

fn futures_ready<T>(fut: std::future::Ready<Result<T, Infallible>>) -> T {
    match fut.into_inner() {
        Ok(v) => v,
        Err(e) => match e {},
    }
}
