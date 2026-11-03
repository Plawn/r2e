use r2e_core::http::header::{HeaderValue, WWW_AUTHENTICATE};
use r2e_core::http::response::{IntoHttpResponse, Response};
use r2e_core::http::StatusCode;
use r2e_core::{Rejection, RejectionKind};

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

/// Stable message prefixes shared by the producers of a `ValidationFailed`
/// / `InvalidToken` message and [`SecurityError::kind`]. A producer builds
/// its message through them (never a bare literal) so the classifier cannot
/// drift from the wording — the message stays the only payload the variants
/// carry (adding a field would break every `match` downstream).
pub(crate) mod msg {
    /// The JWKS entry cannot verify this token: missing/invalid key
    /// components, unsupported `kty`, or `kty`/`alg`/`use`/`key_ops` that
    /// do not fit the token's algorithm.
    pub const SIGNING_KEY: &str = "Signing key unusable: ";
    /// The token's algorithm is not on the validator's allow-list.
    pub const ALGORITHM: &str = "JWT algorithm refused: ";
    /// The JWT header could not be decoded at all.
    pub const HEADER: &str = "Failed to decode header: ";
    /// The JWT header carries no `kid`.
    pub const MISSING_KID: &str = "JWT header missing 'kid' field";
    /// The signature does not verify against the signing key.
    pub const SIGNATURE: &str = "Invalid signature";
    /// The token's `nbf` is in the future.
    pub const NOT_YET_VALID: &str = "Token not yet valid (nbf)";
    /// The token's `sub` does not match the expected subject.
    pub const SUBJECT: &str = "Invalid subject";
}

/// The category of a [`SecurityError`], finer than its variants: what an
/// operator needs to tell a client problem (a forged or stale token) from
/// an IdP-side one (a JWKS entry that cannot verify anything) without
/// parsing the message. `#[non_exhaustive]`: match with a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SecurityErrorKind {
    /// No `Authorization` header.
    MissingAuthHeader,
    /// The `Authorization` scheme is not `Bearer`.
    InvalidAuthScheme,
    /// The token cannot be parsed (header, payload, encoding).
    MalformedToken,
    /// The token's signature does not verify.
    InvalidSignature,
    /// `exp` is in the past.
    TokenExpired,
    /// The JWT header carries no `kid`.
    MissingKid,
    /// The `kid` is not in the JWKS.
    UnknownKid,
    /// The JWKS endpoint could not be reached or parsed (server-side).
    JwksUnavailable,
    /// The token's algorithm is not allowed by the validator.
    AlgorithmRefused,
    /// The JWKS entry for this `kid` cannot verify this token (bad key
    /// material, unsupported `kty`, `kty`/`alg`/`use`/`key_ops` mismatch).
    SigningKeyUnusable,
    /// A claim check failed: issuer, audience, `nbf`, subject, a required
    /// claim.
    Claims,
}

impl SecurityError {
    /// The error's [`SecurityErrorKind`]: the finer category the message
    /// encodes, read from the stable prefixes this crate's producers use.
    pub fn kind(&self) -> SecurityErrorKind {
        match self {
            SecurityError::MissingAuthHeader => SecurityErrorKind::MissingAuthHeader,
            SecurityError::InvalidAuthScheme => SecurityErrorKind::InvalidAuthScheme,
            SecurityError::TokenExpired => SecurityErrorKind::TokenExpired,
            SecurityError::UnknownKeyId(_) => SecurityErrorKind::UnknownKid,
            SecurityError::JwksFetchError(_) => SecurityErrorKind::JwksUnavailable,
            SecurityError::InvalidToken(message) => match message.as_str() {
                msg::MISSING_KID => SecurityErrorKind::MissingKid,
                msg::SIGNATURE => SecurityErrorKind::InvalidSignature,
                msg::NOT_YET_VALID | msg::SUBJECT => SecurityErrorKind::Claims,
                _ => SecurityErrorKind::MalformedToken,
            },
            SecurityError::ValidationFailed(message) => {
                if message.starts_with(msg::SIGNING_KEY) {
                    SecurityErrorKind::SigningKeyUnusable
                } else if message.starts_with(msg::ALGORITHM) {
                    SecurityErrorKind::AlgorithmRefused
                } else {
                    SecurityErrorKind::Claims
                }
            }
        }
    }
}

impl SecurityError {
    /// Whether this is a server-side failure (vs a client authentication failure).
    ///
    /// A failure to reach or parse the JWKS endpoint is the server's problem, not
    /// the client's — it should surface as `503`, not `401`. `pub`: crates
    /// building auth layers on r2e-security (r2e-mcp) apply the same split.
    pub fn is_server_error(&self) -> bool {
        matches!(self, SecurityError::JwksFetchError(_))
    }

    /// HTTP status to return for this error.
    pub fn status(&self) -> StatusCode {
        if self.is_server_error() {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::UNAUTHORIZED
        }
    }

    /// Client-facing message. Never leaks internal details.
    pub fn public_message(&self) -> &'static str {
        if self.is_server_error() {
            "Service unavailable"
        } else {
            "Unauthorized"
        }
    }
}

impl IntoHttpResponse for SecurityError {
    fn into_http_response(self) -> Response {
        // Both bodies are constant (`public_message` returns one of two
        // `&'static str`s), so they are pre-serialized: an unauthenticated
        // request storm is the hot path here, and it should not pay a
        // `serde_json::Value` allocation per rejection.
        let body = if self.is_server_error() {
            r#"{"error":"Service unavailable"}"#
        } else {
            r#"{"error":"Unauthorized"}"#
        };
        r2e_core::http::response::static_json(self.status(), body)
    }
}

r2e_core::http::impl_into_response!(SecurityError);

impl From<SecurityError> for r2e_core::HttpError {
    fn from(err: SecurityError) -> Self {
        r2e_core::HttpError::from_status(err.status(), err.public_message())
    }
}

/// Authentication failures project as `Unauthenticated` (401, with
/// `WWW-Authenticate: Bearer`); a JWKS outage as `Unavailable` (503). The
/// client-facing message is [`SecurityError::public_message`] — the detailed
/// cause travels in [`Rejection::source`] only.
impl From<SecurityError> for Rejection {
    fn from(err: SecurityError) -> Self {
        let kind = if err.is_server_error() {
            RejectionKind::Unavailable
        } else {
            RejectionKind::Unauthenticated
        };
        let mut rejection = Rejection::new(kind, err.public_message());
        if kind == RejectionKind::Unauthenticated {
            rejection = rejection.header(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        rejection.source(err)
    }
}
