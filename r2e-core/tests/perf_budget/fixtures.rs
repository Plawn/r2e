//! Extractors and the request driver shared by the budget modules.

use std::hint::black_box;

use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::header::Parts;
use r2e_core::http::response::Response;
use r2e_core::http::{Body, Request, Routes, StatusCode};
use r2e_core::Identity;
use tower::ServiceExt;

use crate::allocations;

/// Route every fixture controller serves.
pub const PATH: &str = "/budget";

/// An identity extracted with a fixed value: no header read, no
/// cryptography, no allocation. What is left is the framework's plumbing.
pub struct StubIdentity(pub &'static str);

impl Identity for StubIdentity {
    fn sub(&self) -> &str {
        self.0
    }
}

impl<S: Send + Sync> FromRequestParts<S> for StubIdentity {
    type Rejection = Response;

    async fn from_request_parts(_parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(StubIdentity("budget-subject"))
    }
}

/// Two allocation-free request-scoped values, for `#[inject(request)]`.
pub struct Locale(pub &'static str);
pub struct Attempt(pub u8);

impl<S: Send + Sync> FromRequestParts<S> for Locale {
    type Rejection = Response;

    async fn from_request_parts(_parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Locale("en"))
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Attempt {
    type Rejection = Response;

    async fn from_request_parts(_parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Attempt(1))
    }
}

fn request() -> Request<Body> {
    Request::builder().uri(PATH).body(Body::empty()).unwrap()
}

/// Allocations one request costs once the router is warm.
///
/// Requests are built before the measured window, so the count covers the
/// dispatch and the response only. The count must be the same for every
/// request: a remainder means the path is not deterministic and no budget on
/// it would be trustworthy.
pub async fn allocations_per_request(router: &Routes) -> u64 {
    const WARM_UP: usize = 8;
    const MEASURED: u64 = 64;

    for _ in 0..WARM_UP {
        let response = router.clone().oneshot(request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    let requests: Vec<_> = (0..MEASURED).map(|_| request()).collect();
    let before = allocations();
    for request in requests {
        black_box(router.clone().oneshot(request).await.unwrap());
    }
    let total = allocations() - before;

    assert_eq!(
        total % MEASURED,
        0,
        "allocation count varies from one request to the next ({total} over {MEASURED} requests)"
    );
    total / MEASURED
}
