use crate::http::response::{IntoResponse, Response};
use crate::http::{Json, StatusCode};

/// Helper to create a JSON error response with a standard `{ "error": message }` body.
fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    let body = serde_json::json!({ "error": message.into() });
    (status, Json(body)).into_response()
}

pub enum AppError {
    NotFound(String),
    Unauthorized(String),
    Forbidden(String),
    BadRequest(String),
    Internal(String),
    #[cfg(feature = "validation")]
    Validation(crate::validation::ValidationErrorResponse),
    Custom {
        status: StatusCode,
        body: serde_json::Value,
    },
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            #[cfg(feature = "validation")]
            AppError::Validation(resp) => {
                let body = serde_json::json!({
                    "error": "Validation failed",
                    "details": resp.errors,
                });
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
            AppError::Custom { status, body } => {
                (status, Json(body)).into_response()
            }
            other => {
                let (status, message) = match other {
                    AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
                    AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
                    AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
                    AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
                    AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
                    #[cfg(feature = "validation")]
                    AppError::Validation(_) => unreachable!(),
                    AppError::Custom { .. } => unreachable!(),
                };
                error_response(status, message)
            }
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::NotFound(msg) => write!(f, "Not Found: {msg}"),
            AppError::Unauthorized(msg) => write!(f, "Unauthorized: {msg}"),
            AppError::Forbidden(msg) => write!(f, "Forbidden: {msg}"),
            AppError::BadRequest(msg) => write!(f, "Bad Request: {msg}"),
            AppError::Internal(msg) => write!(f, "Internal Error: {msg}"),
            #[cfg(feature = "validation")]
            AppError::Validation(resp) => write!(f, "Validation Error: {} errors", resp.errors.len()),
            AppError::Custom { status, body } => write!(f, "Custom Error ({status}): {body}"),
        }
    }
}

impl std::fmt::Debug for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <Self as std::fmt::Display>::fmt(self, f)
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::Internal(err.to_string())
    }
}

/// Generate `From<E> for AppError` implementations that map error types to
/// a specific `AppError` variant.
///
/// # Example
///
/// ```ignore
/// r2e_core::map_error! {
///     sqlx::Error => Internal,
///     std::io::Error => Internal,
/// }
/// ```
#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    async fn error_parts(err: AppError) -> (StatusCode, serde_json::Value) {
        let resp = err.into_response();
        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        (status, json)
    }

    #[tokio::test]
    async fn app_error_not_found_status() {
        let (status, body) = error_parts(AppError::NotFound("resource missing".into())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "resource missing");
    }

    #[tokio::test]
    async fn app_error_bad_request_status() {
        let (status, body) = error_parts(AppError::BadRequest("invalid input".into())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid input");
    }

    #[tokio::test]
    async fn app_error_unauthorized_status() {
        let (status, body) = error_parts(AppError::Unauthorized("no token".into())).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "no token");
    }

    #[tokio::test]
    async fn app_error_forbidden_status() {
        let (status, body) = error_parts(AppError::Forbidden("access denied".into())).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], "access denied");
    }

    #[tokio::test]
    async fn app_error_internal_status() {
        let (status, body) = error_parts(AppError::Internal("server broke".into())).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"], "server broke");
    }

    #[tokio::test]
    async fn app_error_custom_status_and_body() {
        let custom_body = serde_json::json!({"detail": "teapot"});
        let (status, body) = error_parts(AppError::Custom {
            status: StatusCode::IM_A_TEAPOT,
            body: custom_body.clone(),
        })
        .await;
        assert_eq!(status, StatusCode::IM_A_TEAPOT);
        assert_eq!(body, custom_body);
    }

    #[test]
    fn app_error_display_formatting() {
        assert_eq!(
            AppError::NotFound("x".into()).to_string(),
            "Not Found: x"
        );
        assert_eq!(
            AppError::Unauthorized("y".into()).to_string(),
            "Unauthorized: y"
        );
        assert_eq!(
            AppError::Forbidden("z".into()).to_string(),
            "Forbidden: z"
        );
        assert_eq!(
            AppError::BadRequest("w".into()).to_string(),
            "Bad Request: w"
        );
        assert_eq!(
            AppError::Internal("v".into()).to_string(),
            "Internal Error: v"
        );
    }

    #[test]
    fn app_error_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let app_err: AppError = io_err.into();
        match app_err {
            AppError::Internal(msg) => assert!(msg.contains("file missing")),
            other => panic!("expected Internal, got {other}"),
        }
    }
}

#[macro_export]
macro_rules! map_error {
    ( $( $err_ty:ty => $variant:ident ),* $(,)? ) => {
        $(
            impl From<$err_ty> for $crate::AppError {
                fn from(err: $err_ty) -> Self {
                    $crate::AppError::$variant(err.to_string())
                }
            }
        )*
    };
}
