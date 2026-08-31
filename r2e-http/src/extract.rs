pub use axum::extract::{
    rejection::{FormRejection, PathRejection, QueryRejection},
    ConnectInfo, DefaultBodyLimit, FromRef, FromRequest, FromRequestParts, MatchedPath,
    OptionalFromRequest, OptionalFromRequestParts, OriginalUri, Path, Query, RawPathParams,
    Request, State,
};
pub use axum::Form;

/// Rejections produced by the built-in extractors.
///
/// Re-exported so an application can name the failure type of `Query<T>`,
/// `Path<T>` or `Form<T>` (in a `map_err`, a custom `IntoHttpResponse`, or a
/// `#[derive(ApiError)]` variant) without reaching through
/// [`axum_compat`](crate::axum_compat). `Json<T>` is R2E's own extractor, so
/// its rejection lives with it ([`crate::json::JsonRejection`]).
pub mod rejection {
    pub use super::{FormRejection, PathRejection, QueryRejection};
    pub use crate::json::JsonRejection;
}
