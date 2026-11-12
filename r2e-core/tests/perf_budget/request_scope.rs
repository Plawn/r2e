//! Request scope: `#[inject(identity)]` / `#[inject(request)]` fields are
//! extracted into the per-request façade in place. The façade is a value on
//! the handler's stack, so a request-scoped field costs what its extractor
//! costs and nothing more — here the extractors cost nothing.

use std::hint::black_box;

use r2e_core::http::StatusCode;
use r2e_core::prelude::*;
use r2e_core::AppBuilder;

use crate::fixtures::{allocations_per_request, Attempt, Locale, StubIdentity};

/// Allocations the request façade may add per request, whatever the number
/// of request-scoped fields, over the same controller without any.
const REQUEST_SCOPE_OVERHEAD: u64 = 0;

#[controller]
struct NoRequestScope {
    #[inject]
    label: &'static str,
}

#[routes]
impl NoRequestScope {
    #[get("/budget")]
    async fn handle(&self) -> StatusCode {
        black_box(self.label);
        StatusCode::NO_CONTENT
    }
}

#[controller]
struct OneRequestField {
    #[inject]
    label: &'static str,
    #[inject(request)]
    locale: Locale,
}

#[routes]
impl OneRequestField {
    #[get("/budget")]
    async fn handle(&self) -> StatusCode {
        black_box((self.label, self.locale.0));
        StatusCode::NO_CONTENT
    }
}

#[controller]
struct ThreeRequestFields {
    #[inject]
    label: &'static str,
    #[inject(identity)]
    user: StubIdentity,
    #[inject(request)]
    locale: Locale,
    #[inject(request)]
    attempt: Attempt,
}

#[routes]
impl ThreeRequestFields {
    #[get("/budget")]
    async fn handle(&self) -> StatusCode {
        black_box((self.label, self.user.0, self.locale.0, self.attempt.0));
        StatusCode::NO_CONTENT
    }
}

macro_rules! controller_router {
    ($controller:ty) => {
        AppBuilder::new()
            .provide("ok")
            .build_state()
            .await
            .register_controller::<$controller>()
            .build()
    };
}

#[r2e_core::test(flavor = "current_thread")]
async fn request_scoped_fields_add_no_allocation() {
    let none = allocations_per_request(&controller_router!(NoRequestScope)).await;
    let one = allocations_per_request(&controller_router!(OneRequestField)).await;
    let three = allocations_per_request(&controller_router!(ThreeRequestFields)).await;

    assert!(
        one <= none + REQUEST_SCOPE_OVERHEAD,
        "one #[inject(request)] field: {one} allocations per request, \
         {none} without request scope (budget: +{REQUEST_SCOPE_OVERHEAD})"
    );
    assert!(
        three <= none + REQUEST_SCOPE_OVERHEAD,
        "identity + two #[inject(request)] fields: {three} allocations per request, \
         {none} without request scope (budget: +{REQUEST_SCOPE_OVERHEAD})"
    );
}
