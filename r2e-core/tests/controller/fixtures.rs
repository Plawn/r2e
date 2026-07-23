//! Request-scoped fixtures shared by the controller test modules: an
//! `Identity` (`Subject`, from `x-user`), a plain request-scoped value
//! (`TenantId`, from `x-tenant`), and the request helper built on them.

#![allow(dead_code)]

use http_body_util::BodyExt;
use r2e_core::extract::OptionalFromRequestPartsVia;
use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::response::{IntoResponse, Response};
use r2e_core::http::{Body, StatusCode};
use r2e_core::Identity;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// An identity extracted from the `x-user` header. Implements `Identity` so it
/// can drive guards and `Option<Subject>` (struct-level optional identity).
pub struct Subject(pub String);

impl Identity for Subject {
    fn sub(&self) -> &str {
        &self.0
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Subject {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut r2e_core::http::header::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get("x-user")
            .and_then(|v| v.to_str().ok())
            .map(|s| Subject(s.to_owned()))
            .ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())
    }
}

/// Marker so `Option<Subject>` resolves through a single `ViaOpt` path.
///
/// If `Subject` implemented axum's `OptionalFromRequestParts` instead, the
/// `ViaAxum` bridge would *also* make `Option<Subject>: FromRequestParts` (via
/// axum's blanket `Option<T>` impl), leaving two candidate marker impls for
/// `FromRequestPartsVia` — an ambiguity. Implementing the `Via` trait directly,
/// exactly like real bean-backed identities (`AuthenticatedUser`) do, keeps the
/// optional path unambiguous while `Subject`'s required-identity `FromRequestParts`
/// impl above still bridges through `ViaAxum`.
pub struct SubjectViaOpt;

impl<S: Send + Sync> OptionalFromRequestPartsVia<S, SubjectViaOpt> for Subject {
    type Rejection = Response;

    async fn from_request_parts_via(
        parts: &mut r2e_core::http::header::Parts,
        _state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        Ok(parts
            .headers
            .get("x-user")
            .and_then(|v| v.to_str().ok())
            .map(|s| Subject(s.to_owned())))
    }
}

/// A non-identity request-scoped value from the `x-tenant` header, proving the
/// generic `#[inject(request)]` scope works for any `FromRequestParts` type.
pub struct TenantId(pub String);

impl<S: Send + Sync> FromRequestParts<S> for TenantId {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut r2e_core::http::header::Parts,
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

/// Collect a response body into a `String`.
pub async fn body_string(resp: Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// GET with optional `x-user` / `x-tenant` headers.
pub async fn req(
    router: r2e_core::http::Router,
    path: &str,
    user: Option<&str>,
    tenant: Option<&str>,
) -> (StatusCode, String) {
    let mut headers = Vec::new();
    if let Some(u) = user {
        headers.push(("x-user", u));
    }
    if let Some(t) = tenant {
        headers.push(("x-tenant", t));
    }
    crate::support::send(router, "GET", path, &headers, Body::empty()).await
}

/// Identity that records whether it was ever extracted.
pub struct FlaggingId(pub String);

impl<S: Send + Sync + r2e_core::type_list::BeanLookup> FromRequestParts<S> for FlaggingId {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut r2e_core::http::header::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        state
            .bean_ref::<Arc<AtomicBool>>()
            .expect("identity_ran flag must be provided")
            .store(true, Ordering::SeqCst);
        parts
            .headers
            .get("x-user")
            .and_then(|v| v.to_str().ok())
            .map(|s| FlaggingId(s.to_owned()))
            .ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())
    }
}
