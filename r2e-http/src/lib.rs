//! HTTP abstraction layer for R2E.
//!
//! This crate is the sole owner of the `axum` dependency in the R2E workspace.
//! All other crates access HTTP types through this crate (or via `r2e_core::http`).
//!
//! # What is R2E's, and what is the backend's
//!
//! Most of this crate is a re-export shim: the names are R2E's, the types are
//! axum's. Two things are genuinely R2E's own, and they are the contracts R2E
//! code implements:
//!
//! - [`response::IntoHttpResponse`] + [`impl_into_response!`] — the response
//!   side. R2E error types implement `IntoHttpResponse`; the macro emits the
//!   single bridging impl of the backend's `IntoResponse`.
//! - `r2e_core::web::extract::FromRequestPartsVia` + `Via<T, M>` — the
//!   extraction side (it lives in `r2e-core` because it needs the bean-graph
//!   witness types). Bean-backed extractors implement the R2E trait; `Via` is
//!   the single adapter back to the backend's `FromRequestParts`.
//!
//! Everything else that still speaks the backend's contracts is listed as a
//! named bridge point in `plans/runtime-http-dependency-containment.md` §5.3b.
//!
//! - [`json`] — the JSON codec façade and R2E's own [`Json<T>`]. The one
//!   place that (de)serializes typed values; the codec is a Cargo feature
//!   (`json-sonic`). See `plans/json-codec-containment.md`.
//!
//! [`axum_compat`] is the explicit escape hatch for the rest of axum
//! (§5.3d, decision A).

pub mod axum_compat;
pub mod body;
pub mod extract;
pub mod header;
pub mod json;
pub mod labels;
pub mod middleware;
#[cfg(feature = "multipart")]
pub mod multipart;
#[cfg(feature = "quic")]
pub mod quic;
pub mod response;
pub mod routing;
#[cfg(feature = "ws")]
pub mod ws;

pub use self::body::Body;
pub use self::extract::{
    ConnectInfo, DefaultBodyLimit, Form, FormRejection, FromRef, FromRequest, FromRequestParts,
    MatchedPath, OptionalFromRequest, OptionalFromRequestParts, OriginalUri, Path, PathRejection,
    Query, QueryRejection, RawPathParams, Request, State,
};
pub use self::header::{
    HeaderMap, HeaderName, HeaderValue, Method, Parts, StatusCode, ACCEPT, AUTHORIZATION,
    CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, HOST, LOCATION, ORIGIN, REFERER,
    SET_COOKIE, USER_AGENT,
};
pub use self::json::{Json, JsonError, JsonRejection};
pub use self::response::{
    Html, IntoHttpResponse, IntoResponse, Redirect, Response, Sse, SseEvent, SseKeepAlive,
};
// `http` crate types, re-sourced per plan §5 step 3a — see `header` for why.
pub use axum::serve::ListenerExt;
pub use axum::{serve, Error, Extension, Router};
pub use bytes::Bytes;
pub use http::{Extensions, Uri};
