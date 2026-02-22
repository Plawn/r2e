use r2e::http::StatusCode;
use r2e::prelude::{IntoResponse, Json, Response};

#[derive(Debug)]
pub enum HttpError {
    NotFound(String),
    Database(String),
    Validation(String),
    Internal(String),
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            HttpError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            HttpError::Database(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            HttpError::Validation(msg) => (StatusCode::BAD_REQUEST, msg),
            HttpError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        let body = serde_json::json!({ "error": message });
        (status, Json(body)).into_response()
    }
}

impl From<sqlx::Error> for HttpError {
    fn from(err: sqlx::Error) -> Self {
        HttpError::Database(err.to_string())
    }
}
