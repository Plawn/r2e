//! Bridge-overlap invariant, probe form: a type implementing BOTH axum's
//! `OptionalFromRequestParts` (generically over the state) and R2E's
//! `OptionalFromRequestPartsVia` gives `Option<T>` two extraction routes,
//! so its marker can no longer be inferred. The
//! `assert_unambiguous_extractor` probe is the sanctioned way to detect the
//! violation in the extractor author's tests: it fails right here with the
//! two competing impls listed, instead of an opaque error at every
//! `register_controller()` downstream (that form is pinned in
//! `extractor_dual_route_ambiguous.rs`).

use r2e::extract::{OptionalFromRequestPartsVia, ViaBean, assert_unambiguous_extractor};
use r2e::http::extract::OptionalFromRequestParts;
use r2e::http::header::Parts;
use r2e::type_list::{HCons, HNil, HasBean};
use std::convert::Infallible;

/// The bean backing the R2E-side route.
#[derive(Clone)]
pub struct ApiKeys;

/// The offending extractor: two optional extraction routes.
#[derive(Clone)]
pub struct ApiKey(pub String);

// Route 1: plain axum optional extractor (bridged via `ViaAxum`).
impl<S: Send + Sync> OptionalFromRequestParts<S> for ApiKey {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> Result<Option<Self>, Infallible> {
        Ok(parts
            .headers
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .map(|v| ApiKey(v.to_string())))
    }
}

// Route 2: bean-backed R2E optional extractor (reached via `ViaOpt`).
impl<S, I> OptionalFromRequestPartsVia<S, ViaBean<I>> for ApiKey
where
    S: HasBean<ApiKeys, I> + Send + Sync,
    I: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts_via(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Option<Self>, Infallible> {
        let ApiKeys = state.get_bean();
        Ok(parts
            .headers
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .map(|v| ApiKey(v.to_string())))
    }
}

fn main() {
    type S = HCons<ApiKeys, HNil>;
    assert_unambiguous_extractor::<S, Option<ApiKey>, _>();
}
