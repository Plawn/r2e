//! Guards read the peer address snapshotted before extraction: an
//! `#[inject(request)]` extractor that consumes `ConnectInfo` from the head
//! does not hide the caller's address from the guards that run after it.

use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::header::Parts;
use r2e_core::http::{Body, ConnectInfo, Request, StatusCode};
use r2e_core::prelude::*;
use r2e_core::{Guard, GuardContext, Identity};
use http_body_util::BodyExt;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

type Seen = Arc<Mutex<Vec<Option<SocketAddr>>>>;

/// A request extractor that takes `ConnectInfo` out of the head.
struct TakesConnectInfo(Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for TakesConnectInfo {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(TakesConnectInfo(
            parts
                .extensions
                .remove::<ConnectInfo<SocketAddr>>()
                .map(|info| info.0),
        ))
    }
}

struct PeerProbe;

struct PeerProbeReady {
    seen: Seen,
}

impl r2e_core::DecoratorSpec for PeerProbe {
    type Product = PeerProbeReady;
    type Deps = r2e_core::type_list::TCons<Seen, r2e_core::type_list::TNil>;

    fn build(self, ctx: &r2e_core::BeanContext) -> PeerProbeReady {
        PeerProbeReady { seen: ctx.get() }
    }
}

impl<I: Identity> Guard<I> for PeerProbeReady {
    fn check(&self, ctx: &GuardContext<'_, I>) -> impl Future<Output = Result<(), Rejection>> + Send {
        self.seen.lock().unwrap().push(ctx.peer_addr);
        std::future::ready(Ok(()))
    }
}

#[controller]
struct PeerController {
    #[inject(request)]
    taken: TakesConnectInfo,
}

#[routes]
impl PeerController {
    #[get("/peer")]
    #[guard(PeerProbe)]
    async fn peer(&self) -> String {
        format!("{:?}", self.taken.0)
    }
}

#[r2e_core::test]
async fn guard_sees_the_peer_address_an_extractor_removed() {
    let seen: Seen = Arc::default();
    let router = r2e_core::AppBuilder::new()
        .provide(seen.clone())
        .build_state()
        .await
        .register_controller::<PeerController>()
        .build();

    let peer: SocketAddr = "198.51.100.7:5150".parse().unwrap();
    let mut request = Request::builder().uri("/peer").body(Body::empty()).unwrap();
    request.extensions_mut().insert(ConnectInfo(peer));
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // The extractor did take it out of the head …
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body, format!("{:?}", Some(peer)).as_bytes());
    // … and the guard still saw the address captured before extraction.
    assert_eq!(*seen.lock().unwrap(), vec![Some(peer)]);
}
