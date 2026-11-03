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
    let (parts, message) = into_head(request(), fallback);
    assert_eq!(message, "msg");
    assert_eq!(parts.method, Method::POST);
    assert_eq!(parts.version, http::Version::HTTP_2);
    assert_eq!(parts.uri.path(), "/greeter.Greeter/SayHello");
    assert_eq!(parts.headers.get("authorization").unwrap(), "Bearer t0k");
    assert_eq!(parts.headers.get("x-tenant-id").unwrap(), "acme");
    // No peer without a transport, and never a metadata copy.
    assert!(parts.extensions.get::<ConnectInfo<SocketAddr>>().is_none());
    assert!(parts.extensions.get::<MetadataMap>().is_none());
}

#[test]
fn captured_method_path_wins_over_the_fallback() {
    let mut request = request();
    request
        .extensions_mut()
        .insert(MethodPath(Uri::from_static("/pkg.Svc/Real")));
    let (parts, _) = into_head(request, || panic!("fallback not needed"));
    assert_eq!(parts.uri.path(), "/pkg.Svc/Real");
    assert!(parts.extensions.get::<MethodPath>().is_none(), "consumed");
}

#[test]
fn round_trip_keeps_extractor_edits_and_caller_owned_extensions() {
    let mut request = request();
    // A caller-owned `MetadataMap` extension (e.g. from an interceptor) is
    // the caller's data: moved through untouched, never confused with the
    // request metadata.
    let mut owned = MetadataMap::new();
    owned.insert("x-owned", "yes".parse().unwrap());
    request.extensions_mut().insert(owned);
    request.extensions_mut().insert("caller-tag");

    let (mut parts, message) = into_head(request, fallback);
    assert_eq!(
        parts.extensions.get::<MetadataMap>().unwrap().get("x-owned").unwrap(),
        "yes"
    );

    // Extractors may edit the head; the rebuilt request sees their edits.
    parts.headers.insert("x-added", "1".parse().unwrap());
    parts.extensions.insert(7u32);
    let request = from_head(parts, message);
    assert_eq!(request.get_ref(), &"msg");
    assert_eq!(request.metadata().get("x-added").unwrap(), "1");
    assert_eq!(request.metadata().get("authorization").unwrap(), "Bearer t0k");
    assert_eq!(request.extensions().get::<u32>(), Some(&7));
    assert_eq!(request.extensions().get::<&str>(), Some(&"caller-tag"));
    let owned = request.extensions().get::<MetadataMap>().unwrap();
    assert_eq!(owned.get("x-owned").unwrap(), "yes");
    assert!(owned.get("authorization").is_none());
}

fn with_peer(mut request: Request<&'static str>, peer: SocketAddr) -> Request<&'static str> {
    request
        .extensions_mut()
        .insert(tonic::transport::server::TcpConnectInfo {
            local_addr: None,
            remote_addr: Some(peer),
        });
    request
}

#[test]
fn connect_info_comes_from_the_peer_address() {
    let peer: SocketAddr = "10.0.0.1:4000".parse().unwrap();
    let (parts, _) = into_head(with_peer(request(), peer), fallback);
    assert_eq!(parts.extensions.get::<ConnectInfo<SocketAddr>>().unwrap().0, peer);
}

#[test]
fn pre_existing_connect_info_is_not_overwritten() {
    let peer: SocketAddr = "10.0.0.1:4000".parse().unwrap();
    let forwarded: SocketAddr = "203.0.113.9:443".parse().unwrap();
    let mut request = with_peer(request(), peer);
    request.extensions_mut().insert(ConnectInfo(forwarded));
    let (parts, _) = into_head(request, fallback);
    assert_eq!(
        parts.extensions.get::<ConnectInfo<SocketAddr>>().unwrap().0,
        forwarded
    );
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
