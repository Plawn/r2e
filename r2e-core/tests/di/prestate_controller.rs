//! A controller registered on the pre-state builder and again after
//! `build_state()` (#1215, codex E2): pre-state controller registrations do
//! not survive the transition — routes, lifecycle queues and the per-type
//! core store alike — so the typed registration is the first one and wires
//! the lifecycle exactly once.
//!
//! Only a hand-written `Controller` impl generic over the state can be
//! registered on `AppBuilder<NoState>`; generated impls reject `NoState`.

use http_body_util::BodyExt;
use r2e_core::beans::{BeanContext, BootError};
use r2e_core::http::Routes;
use r2e_core::type_list::TNil;
use r2e_core::{AppBuilder, Controller, RegisterController};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

/// Only this test uses this controller, so a static counter is race-free.
static INITS: AtomicUsize = AtomicUsize::new(0);

struct PreState;

impl<S> Controller<S> for PreState
where
    S: Clone + Send + Sync + 'static,
{
    type Deps = TNil;

    fn construct(_state: &S, _ctx: &BeanContext) -> Self {
        PreState
    }

    fn routes(_state: &S, _core: Arc<Self>, _ctx: &BeanContext) -> Routes<S> {
        Routes::new().route(
            "/prestate",
            r2e_core::http::routing::get(|_: S, _| async { "mounted" }),
        )
    }

    fn post_construct(
        _core: Arc<Self>,
    ) -> Pin<Box<dyn Future<Output = Result<(), BootError>> + Send>> {
        Box::pin(async {
            INITS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[r2e_core::test]
async fn a_controller_registered_before_and_after_build_state_is_wired_once() {
    let router = AppBuilder::new()
        .register_controller::<PreState>()
        .build_state()
        .await
        .register_controller::<PreState>()
        .build_with_consumers()
        .await;

    assert_eq!(INITS.load(Ordering::SeqCst), 1, "post_construct runs");

    let req = r2e_core::http::Request::builder()
        .uri("/prestate")
        .body(r2e_core::http::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), r2e_core::http::StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"mounted");
}
