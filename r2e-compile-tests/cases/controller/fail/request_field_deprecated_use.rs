//! The counterpart of `cases/controller/pass/request_field_attrs.rs`: a
//! `#[deprecated]` request-scoped field really is deprecated where the USER
//! reads it. Only the framework's own accesses carry `#[allow(deprecated)]`
//! (task #985).
#![deny(deprecated)]

use r2e::prelude::*;

#[derive(Clone)]
pub struct Svc;

pub struct TraceId(pub String);

impl<S: Send + Sync> FromRequestParts<S> for TraceId {
    type Rejection = Response;

    async fn from_request_parts(
        _parts: &mut r2e::http::header::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(TraceId(String::new()))
    }
}

#[controller(path = "/attrs")]
pub struct AttrController {
    #[inject]
    svc: Svc,
    #[deprecated(note = "use the trace header instead")]
    #[inject(request)]
    legacy_trace: TraceId,
}

#[routes]
impl AttrController {
    #[get("/")]
    async fn list(&self) -> String {
        self.legacy_trace.0.clone()
    }
}

fn main() {}
