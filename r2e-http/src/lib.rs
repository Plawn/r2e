//! HTTP abstraction layer for R2E.
//!
//! This crate is the sole owner of the `axum` dependency in the R2E workspace.
//! All other crates access HTTP types through this crate (or via `r2e_core::http`).

pub mod body;
pub mod extract;
pub mod header;
pub mod middleware;
pub mod response;
pub mod routing;
#[cfg(feature = "ws")]
pub mod ws;
#[cfg(feature = "multipart")]
pub mod multipart;

pub use axum::{serve, Extension, Json, Router, Error};
pub use axum::http::Uri;
pub use bytes::Bytes;
pub use self::extract::{
    ConnectInfo, DefaultBodyLimit, Form, FromRef, FromRequest, FromRequestParts,
    MatchedPath, OptionalFromRequestParts, OriginalUri, Path, Query, RawPathParams,
    Request, State,
};
pub use self::header::{
    HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Parts,
    ACCEPT, AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, HOST,
    LOCATION, ORIGIN, REFERER, SET_COOKIE, USER_AGENT,
};
pub use self::response::{Html, IntoResponse, Redirect, Response, Sse, SseEvent, SseKeepAlive};
pub use self::body::Body;
