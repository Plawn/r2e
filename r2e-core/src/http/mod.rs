pub use r2e_http::axum_compat;
pub use r2e_http::header;
pub use r2e_http::{body, extract, labels, middleware, response, routing};
#[cfg(feature = "ws")]
pub mod ws;
#[cfg(feature = "multipart")]
pub use r2e_http::multipart;
#[cfg(feature = "quic")]
pub use r2e_http::quic;

pub use r2e_http::{
    serve, Body, Bytes, ConnectInfo, DefaultBodyLimit, Error, Extension, Extensions, Form,
    FormRejection, FromRef, FromRequest, FromRequestParts, HeaderMap, HeaderName, HeaderValue,
    Html, IntoHttpResponse, IntoResponse, Json, JsonRejection, ListenerExt, MatchedPath, Method,
    OptionalFromRequestParts, OriginalUri, Parts, Path, PathRejection, Query, QueryRejection,
    RawPathParams, Redirect, Request, Response, Router, Sse, SseEvent, SseKeepAlive, State,
    StatusCode, Uri, ACCEPT, AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, COOKIE,
    HOST, LOCATION, ORIGIN, REFERER, SET_COOKIE, USER_AGENT,
};

/// Bridge an [`IntoHttpResponse`] type to the HTTP backend's response
/// contract — see [`r2e_http::response`] for why this is a macro.
pub use r2e_http::impl_into_response;
