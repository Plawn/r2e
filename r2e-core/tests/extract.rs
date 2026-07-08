//! Bridge-overlap invariant probes for the extraction layer
//! (`src/extract.rs`).
//!
//! `assert_unambiguous_extractor` compiles iff a type has exactly one
//! extraction route (one inferable marker) against a given state. These
//! tests pin the two sanctioned routes — the blanket `ViaAxum` bridge for
//! plain axum extractors and direct `ViaBean` impls for bean-backed
//! extractors — including their `Option<T>` forms, whose marker selection is
//! the historically fragile part (see the module docs in `src/extract.rs`).
//! The negative case (a type with BOTH routes → E0283) is pinned in
//! `r2e-compile-tests/compile-fail/extractor_dual_route_*.rs`.

use std::collections::HashMap;
use std::convert::Infallible;

use r2e_core::extract::{
    FromRequestPartsVia, OptionalFromRequestPartsVia, ViaBean, assert_unambiguous_extractor,
};
use r2e_core::http::extract::{MatchedPath, Query};
use r2e_core::http::header::{HeaderMap, Parts};
use r2e_core::type_list::{HCons, HNil, HasBean};

/// A bean backing the test extractor.
#[derive(Clone)]
pub struct TenantRegistry;

/// A bean-backed request-scoped extractor, written exactly like
/// `AuthenticatedUser`: R2E's `*Via` traits only, witness parked in
/// `ViaBean`, no axum impls.
#[derive(Clone)]
pub struct TenantId(pub String);

impl<S, I> FromRequestPartsVia<S, ViaBean<I>> for TenantId
where
    S: HasBean<TenantRegistry, I> + Send + Sync,
    I: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts_via(_parts: &mut Parts, state: &S) -> Result<Self, Infallible> {
        let TenantRegistry = state.get_bean();
        Ok(TenantId("tenant".to_string()))
    }
}

impl<S, I> OptionalFromRequestPartsVia<S, ViaBean<I>> for TenantId
where
    S: HasBean<TenantRegistry, I> + Send + Sync,
    I: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts_via(
        _parts: &mut Parts,
        state: &S,
    ) -> Result<Option<Self>, Infallible> {
        let TenantRegistry = state.get_bean();
        Ok(Some(TenantId("tenant".to_string())))
    }
}

type TestState = HCons<TenantRegistry, HNil>;

#[test]
fn bean_backed_extractor_route_is_unambiguous() {
    assert_unambiguous_extractor::<TestState, TenantId, _>();
    assert_unambiguous_extractor::<TestState, Option<TenantId>, _>();
}

#[test]
fn plain_axum_extractor_route_is_unambiguous() {
    assert_unambiguous_extractor::<TestState, HeaderMap, _>();
    assert_unambiguous_extractor::<TestState, Query<HashMap<String, String>>, _>();
    // `Option<T>` for a plain axum extractor resolves through axum's own
    // `FromRequestParts for Option<T> where T: OptionalFromRequestParts`
    // under the `ViaAxum` bridge (NOT through `ViaOpt`).
    assert_unambiguous_extractor::<TestState, Option<MatchedPath>, _>();
}
