use r2e_core::http::response::{IntoResponse, Response};
use r2e_core::http::StatusCode;

/// Security-related errors for JWT validation and authentication.
#[derive(Debug)]
pub enum SecurityError {
    /// The Authorization header is missing from the request.
    MissingAuthHeader,

    /// The authorization scheme is not "Bearer".
    InvalidAuthScheme,

    /// The JWT token is invalid (malformed, bad signature, etc.).
    InvalidToken(String),

    /// The JWT token has expired.
    TokenExpired,

    /// The key ID (kid) from the JWT header is not found in the JWKS.
    UnknownKeyId(String),

    /// Failed to fetch the JWKS from the remote endpoint.
    JwksFetchError(String),

    /// Token validation failed (issuer, audience, or other claim mismatch).
    ValidationFailed(String),
}

impl std::fmt::Display for SecurityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecurityError::MissingAuthHeader => write!(f, "Missing Authorization header"),
            SecurityError::InvalidAuthScheme => write!(f, "Invalid authorization scheme"),
            SecurityError::InvalidToken(msg) => write!(f, "Invalid token: {msg}"),
            SecurityError::TokenExpired => write!(f, "Token expired"),
            SecurityError::UnknownKeyId(kid) => write!(f, "Unknown signing key: {kid}"),
            SecurityError::JwksFetchError(msg) => write!(f, "JWKS fetch error: {msg}"),
            SecurityError::ValidationFailed(msg) => write!(f, "Token validation failed: {msg}"),
        }
    }
}

impl std::error::Error for SecurityError {}

impl SecurityError {
    pub fn public_message(&self) -> &'static str {
        "Unauthorized"
    }
}

impl IntoResponse for SecurityError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.public_message() });
        (StatusCode::UNAUTHORIZED, r2e_core::http::Json(body)).into_response()
    }
}

impl From<SecurityError> for r2e_core::HttpError {
    fn from(err: SecurityError) -> Self {
        r2e_core::HttpError::Unauthorized(err.public_message().to_string())
    }
}
