use r2e::prelude::*;

/// Application-level error type using `#[derive(ApiError)]`.
///
/// Demonstrates best practices for custom error types in R2E:
/// - `#[from]` for automatic conversion from library errors
/// - Explicit status codes and messages per variant
/// - `#[error(transparent)]` to delegate to `HttpError`
#[derive(Debug, ApiError)]
pub enum AppError {
    /// Database errors — mapped from `sqlx::Error` via `#[from]`.
    #[error(status = INTERNAL_SERVER_ERROR, message = "Database error")]
    Database(#[from] sqlx::Error),

    /// Resource not found.
    #[error(status = NOT_FOUND, message = "{0}")]
    NotFound(String),

    /// Bad request / invalid input.
    #[error(status = BAD_REQUEST, message = "{0}")]
    BadRequest(String),

    /// Delegate to the framework's `HttpError` for everything else.
    #[error(transparent)]
    Http(#[from] HttpError),
}

// `AppError` is also an **error envelope**: `#[error(transparent)] Http(#[from] HttpError)`
// gives it `From<Rejection>` and `ErrorSchema` through `HttpError`, so a
// `Result<T, AppError>` route renders framework failures (bad body, failed
// identity, denied guard, …) exactly like `HttpError` would — `{"error": ".."}`.
// `app.rs` also installs it as the app-level projection
// (`.error_projection::<AppError>()`), the envelope of every route whose
// return type is not a `Result<T, E>` envelope, and of 404/405/413/panics.

/// An RFC 9457 *problem details* envelope — a wire shape that is **not** the
/// plain `{"error": ..}` body.
///
/// This is the hand-written form of an error envelope: three impls, no
/// attribute anywhere. A route that returns `Result<T, Problem>` answers
/// **every** failure in this shape — its own `Err(Problem)`, but also a
/// malformed body, a missing `content-type`, a bad path segment, a missing
/// token or a garde report (see `ProblemController`).
///
/// - [`From<Rejection>`] builds the body from the hub value and **reads
///   `rejection.status`** (already remapped by `status_of`).
/// - [`IntoHttpResponse`] renders it as `application/problem+json`.
/// - [`ErrorSchema`] is the static side shared by the runtime and the OpenAPI
///   builder: garde reports are answered with 422 instead of the default 400,
///   and the spec documents the `Problem` component on 422 — never 400.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct Problem {
    /// Problem type URI; `about:blank` means `title` is the HTTP reason phrase.
    #[serde(rename = "type")]
    pub kind: String,
    /// Short human-readable summary (the status' canonical reason).
    pub title: String,
    /// HTTP status, repeated in the body as RFC 9457 recommends.
    pub status: u16,
    /// Human-readable explanation specific to this occurrence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Machine-readable rejection kind (`InvalidBody`, `Unauthenticated`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Field-level errors (garde validation reports).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<serde_json::Value>,
}

impl Problem {
    pub fn new(status: StatusCode, detail: impl Into<String>) -> Self {
        Self {
            kind: "about:blank".to_string(),
            title: status.canonical_reason().unwrap_or("Error").to_string(),
            status: status.as_u16(),
            detail: Some(detail.into()),
            code: None,
            errors: None,
        }
    }

    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, detail)
    }
}

impl From<Rejection> for Problem {
    fn from(r: Rejection) -> Self {
        // `r.status` is authoritative: `status_of` has already been applied and
        // a fault may carry its own status (a 413 body limit, a 503 JWKS outage).
        let mut problem = Problem::new(r.status, r.message.into_owned());
        problem.code = Some(format!("{:?}", r.kind));
        problem.errors = r.details;
        problem
    }
}

impl IntoHttpResponse for Problem {
    fn into_http_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut resp = (status, Json(self)).into_response();
        resp.headers_mut().insert(
            r2e::http::header::CONTENT_TYPE,
            r2e::http::header::HeaderValue::from_static("application/problem+json"),
        );
        resp
    }
}

// Bridges to the HTTP backend's response contract (what makes
// `Result<T, Problem>` returnable from a handler).
r2e::http::impl_into_response!(Problem);

impl ErrorSchema for Problem {
    /// Garde reports answer 422; every other kind keeps its default status.
    /// The OpenAPI builder calls this same function, so the spec says 422 too.
    fn status_of(kind: RejectionKind) -> StatusCode {
        match kind {
            RejectionKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
            other => other.default_status(),
        }
    }

    /// One body shape for every status: the `Problem` component.
    fn body_schema() -> Option<(String, serde_json::Value)> {
        let schema = serde_json::to_value(schemars::schema_for!(Problem)).ok()?;
        Some(("Problem".to_string(), schema))
    }
}
