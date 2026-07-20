use r2e_core::http::response::{IntoResponse, Response};
use r2e_core::http::Json;
use r2e_core::http::{header, HeaderMap, HeaderValue, StatusCode};
use serde::Serialize;

/// OAuth 2.0 error response per RFC 6749 Section 5.2.
#[derive(Debug, Serialize)]
pub struct OidcErrorBody {
    pub error: &'static str,
    pub error_description: String,
}

/// Local token issuer error type.
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
    /// Invalid Bearer token.
    InvalidToken(String),
    /// Insufficient scope for the requested resource.
    InsufficientScope(String),
    /// Invalid server configuration.
    Configuration(String),
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
            OidcError::Unauthorized(_) => "invalid_token",
            OidcError::InvalidToken(_) => "invalid_token",
            OidcError::InsufficientScope(_) => "insufficient_scope",
            OidcError::Configuration(_) => "server_error",
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
            OidcError::InvalidToken(_) => StatusCode::UNAUTHORIZED,
            OidcError::InsufficientScope(_) => StatusCode::FORBIDDEN,
            OidcError::Configuration(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OidcError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn public_description(&self) -> &str {
        match self {
            OidcError::InvalidRequest(s)
            | OidcError::InvalidGrant(s)
            | OidcError::UnsupportedGrantType(s)
            | OidcError::InvalidClient(s)
            | OidcError::Unauthorized(s)
            | OidcError::InvalidToken(s)
            | OidcError::InsufficientScope(s) => s,
            OidcError::Configuration(_) | OidcError::Internal(_) => "server error",
        }
    }

    fn www_authenticate(&self) -> Option<HeaderValue> {
        let value = match self {
            OidcError::InvalidClient(_) => r#"Basic realm="r2e-oidc", error="invalid_client""#,
            OidcError::Unauthorized(_) | OidcError::InvalidToken(_) => {
                r#"Bearer realm="r2e-oidc", error="invalid_token""#
            }
            OidcError::InsufficientScope(_) => {
                r#"Bearer realm="r2e-oidc", error="insufficient_scope""#
            }
            _ => return None,
        };
        Some(HeaderValue::from_static(value))
    }
}

impl IntoResponse for OidcError {
    fn into_response(self) -> Response {
        let mut headers = HeaderMap::new();
        if let Some(value) = self.www_authenticate() {
            headers.insert(header::WWW_AUTHENTICATE, value);
        }

        let body = OidcErrorBody {
            error: self.error_code(),
            error_description: self.public_description().to_string(),
        };
        (self.status_code(), headers, Json(body)).into_response()
    }
}

impl std::fmt::Display for OidcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let description = match self {
            OidcError::InvalidRequest(s)
            | OidcError::InvalidGrant(s)
            | OidcError::UnsupportedGrantType(s)
            | OidcError::InvalidClient(s)
            | OidcError::Unauthorized(s)
            | OidcError::InvalidToken(s)
            | OidcError::InsufficientScope(s)
            | OidcError::Configuration(s)
            | OidcError::Internal(s) => s,
        };
        write!(f, "{}: {}", self.error_code(), description)
    }
}

impl std::error::Error for OidcError {}
