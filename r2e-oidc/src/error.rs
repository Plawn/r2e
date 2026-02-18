use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// OAuth 2.0 error response per RFC 6749 Section 5.2.
#[derive(Debug, Serialize)]
pub struct OidcErrorBody {
    pub error: &'static str,
    pub error_description: String,
}

/// OIDC server error type.
#[derive(Debug)]
pub enum OidcError {
    /// Invalid grant type or missing required parameters.
    InvalidRequest(String),
    /// Invalid username/password or client credentials.
    InvalidGrant(String),
    /// Unsupported grant type.
    UnsupportedGrantType(String),
    /// Invalid client credentials.
    InvalidClient(String),
    /// Unauthorized (missing or invalid Bearer token).
    Unauthorized(String),
    /// Internal server error.
    Internal(String),
}

impl OidcError {
    fn error_code(&self) -> &'static str {
        match self {
            OidcError::InvalidRequest(_) => "invalid_request",
            OidcError::InvalidGrant(_) => "invalid_grant",
            OidcError::UnsupportedGrantType(_) => "unsupported_grant_type",
            OidcError::InvalidClient(_) => "invalid_client",
            OidcError::Unauthorized(_) => "unauthorized",
            OidcError::Internal(_) => "server_error",
        }
    }

    fn status_code(&self) -> StatusCode {
        match self {
            OidcError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            OidcError::InvalidGrant(_) => StatusCode::BAD_REQUEST,
            OidcError::UnsupportedGrantType(_) => StatusCode::BAD_REQUEST,
            OidcError::InvalidClient(_) => StatusCode::UNAUTHORIZED,
            OidcError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            OidcError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn description(&self) -> &str {
        match self {
            OidcError::InvalidRequest(s)
            | OidcError::InvalidGrant(s)
            | OidcError::UnsupportedGrantType(s)
            | OidcError::InvalidClient(s)
            | OidcError::Unauthorized(s)
            | OidcError::Internal(s) => s,
        }
    }
}

impl IntoResponse for OidcError {
    fn into_response(self) -> Response {
        let body = OidcErrorBody {
            error: self.error_code(),
            error_description: self.description().to_string(),
        };
        (self.status_code(), Json(body)).into_response()
    }
}

impl std::fmt::Display for OidcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.error_code(), self.description())
    }
}

impl std::error::Error for OidcError {}
