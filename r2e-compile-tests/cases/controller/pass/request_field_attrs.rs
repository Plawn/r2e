//! Attributes written on a request-scoped controller field are projected onto
//! the generated request-data extractor and the façade — and the framework code
//! that binds them is immune to what they turn on (task #985).
//!
//! Both halves are load-bearing here:
//! - `#![deny(non_snake_case)]` + a field-level `#[allow(non_snake_case)]`: the
//!   field is *removed* from the physical core, so the only `correlationId`
//!   declarations left are the generated ones. If the macro dropped the
//!   `#[allow]`, this file would fail — that is the positive projection proof.
//! - `#![deny(deprecated)]` + a `#[deprecated]` field: the generated extraction,
//!   the façade binding and `guard_identity` all touch it, so without
//!   `#[allow(deprecated)]` on the generated items the deprecation would fire
//!   inside framework code and the user could never opt out. Reading the field
//!   from a route body still warns — that is
//!   `cases/controller/fail/request_field_deprecated_use.rs`.
#![deny(deprecated, non_snake_case)]

use r2e::prelude::*;

#[derive(Clone)]
pub struct Svc;

pub struct TraceId(pub String);

impl<S: Send + Sync> FromRequestParts<S> for TraceId {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut r2e::http::header::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        let id = parts
            .headers
            .get("x-trace")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("none")
            .to_owned();
        Ok(TraceId(id))
    }
}

#[controller(path = "/attrs")]
pub struct AttrController {
    #[inject]
    svc: Svc,
    /// Doc comments ride along too.
    #[deprecated(note = "use the trace header instead")]
    #[inject(request)]
    legacy_trace: TraceId,
    #[allow(non_snake_case)]
    #[inject(request)]
    correlationId: TraceId,
}

#[routes]
impl AttrController {
    #[get("/")]
    async fn list(&self) -> String {
        // Reads the non-deprecated field only: nothing here should trip
        // `deny(deprecated)`, and neither should any generated access.
        self.correlationId.0.clone()
    }
}

fn main() {}
