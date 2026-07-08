//! Bridge-overlap invariant, real-world form: a controller using an
//! `Option<T>` field whose `T` has TWO extraction routes (a generic axum
//! `OptionalFromRequestParts` impl AND an R2E `OptionalFromRequestPartsVia`
//! impl) fails marker inference at `register_controller()`. This pins the
//! opaque `E0283` an app author sees when a third-party extractor violates
//! the invariant — the actionable diagnosis lives in the extractor author's
//! probe (`extractor_dual_route_probe.rs`).

use r2e::extract::{OptionalFromRequestPartsVia, ViaBean};
use r2e::http::extract::OptionalFromRequestParts;
use r2e::http::header::Parts;
use r2e::prelude::*;
use r2e::type_list::HasBean;
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

#[controller(path = "/k")]
pub struct KeyController {
    #[inject(request)]
    key: Option<ApiKey>,
}

#[routes]
impl KeyController {
    #[get("/")]
    async fn show(&self) -> String {
        format!("{}", self.key.is_some())
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .provide(ApiKeys)
            .build_state()
            .await
            .register_controller::<KeyController>()
    };
}
