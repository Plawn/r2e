//! Phase 4: the generic `#[inject(request)]` scope — any `FromRequestParts`
//! type can be a request-scoped controller field, moved onto the façade and
//! read via `self.<field>` from a route method. Multiple request fields plus an
//! identity field coexist; injected core fields remain reachable via `Deref`.

use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState {
    pub label: String,
}

/// A request-scoped value extracted from a header.
pub struct TenantId(pub String);

impl<S: Send + Sync> FromRequestParts<S> for TenantId {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut r2e::http::header::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get("x-tenant")
            .and_then(|v| v.to_str().ok())
            .map(|s| TenantId(s.to_owned()))
            .ok_or_else(|| StatusCode::BAD_REQUEST.into_response())
    }
}

/// A second request-scoped value, proving multiple `#[inject(request)]` fields.
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

#[controller(path = "/req", state = AppState)]
pub struct RequestScopeController {
    #[inject]
    label: String,
    #[inject(request)]
    tenant: TenantId,
    #[inject(request)]
    trace: TraceId,
}

#[routes]
impl RequestScopeController {
    #[get("/info")]
    async fn info(&self) -> String {
        // `self.tenant` / `self.trace` are façade fields; `self.label` is a core
        // field reached through `Deref`.
        format!("{}:{}:{}", self.label, self.tenant.0, self.trace.0)
    }
}

fn main() {}
