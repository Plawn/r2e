//! Auth-layer errors and `WWW-Authenticate` challenge building.

use r2e_core::http::header::{self, HeaderValue};
use r2e_core::http::{Response, StatusCode};

/// Why a bearer token was not accepted.
///
/// Rendered by the auth layer as an OAuth-shaped HTTP error (RFC 6750): 401
/// with a `WWW-Authenticate: Bearer …` challenge, 403 for
/// `insufficient_scope`, 503 when the upstream IdP is unreachable (no
/// challenge — an IdP outage must not send clients into a re-auth loop).
#[derive(Debug, Clone)]
pub enum McpAuthError {
    /// No `Authorization: Bearer …` header (401; the challenge carries
    /// `resource_metadata` so the client can discover the IdP).
    MissingToken,
    /// The token failed validation (401, `error="invalid_token"`). The
    /// message is a static, allow-listed reason — never token contents.
    InvalidToken(&'static str),
    /// The token lacks the server-wide `required-scopes` (403,
    /// `error="insufficient_scope"`).
    InsufficientScope {
        /// The scopes that were required and missing.
        missing: Vec<String>,
    },
    /// The `Origin` header is not in `mcp.allowed-origins` (403).
    InvalidOrigin,
    /// The validator could not reach the IdP (JWKS/discovery/introspection
    /// outage) — 503, no challenge.
    Upstream(String),
}

impl McpAuthError {
    /// The static reason string used in `error_description`.
    pub fn description(&self) -> &str {
        match self {
            McpAuthError::MissingToken => "missing bearer token",
            McpAuthError::InvalidToken(reason) => reason,
            McpAuthError::InsufficientScope { .. } => "token lacks a required scope",
            McpAuthError::InvalidOrigin => "origin not allowed",
            McpAuthError::Upstream(_) => "authorization server unavailable",
        }
    }
}

impl std::fmt::Display for McpAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The one variant whose detail is internal-facing (log/boot
            // messages) rather than client-facing.
            McpAuthError::Upstream(detail) => {
                write!(f, "authorization server unavailable: {detail}")
            }
            other => f.write_str(other.description()),
        }
    }
}

impl std::error::Error for McpAuthError {}

/// Escape a value for use inside an RFC 9110 quoted-string.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            // Control characters are not representable; drop them.
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Build a `WWW-Authenticate: Bearer …` header value.
///
/// Every parameter value goes through RFC 9110 quoted-string escaping.
/// `params` pairs with an empty value are skipped.
pub fn www_authenticate(params: &[(&str, &str)]) -> HeaderValue {
    let mut value = String::from("Bearer");
    let mut first = true;
    for (k, v) in params {
        if v.is_empty() {
            continue;
        }
        value.push_str(if first { " " } else { ", " });
        first = false;
        value.push_str(k);
        value.push('=');
        value.push_str(&quote(v));
    }
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("Bearer"))
}

/// Render an auth failure as the OAuth-shaped HTTP response.
///
/// `challenge` is the prebuilt base challenge (carrying `resource_metadata`
/// and advertised scopes); scope/description parameters are appended per
/// error.
pub(crate) fn auth_error_response(error: &McpAuthError, resource_metadata_url: &str) -> Response {
    let (status, oauth_code) = match error {
        McpAuthError::MissingToken => (StatusCode::UNAUTHORIZED, None),
        McpAuthError::InvalidToken(_) => (StatusCode::UNAUTHORIZED, Some("invalid_token")),
        McpAuthError::InsufficientScope { .. } => {
            (StatusCode::FORBIDDEN, Some("insufficient_scope"))
        }
        McpAuthError::InvalidOrigin => (StatusCode::FORBIDDEN, Some("invalid_origin")),
        McpAuthError::Upstream(_) => (StatusCode::SERVICE_UNAVAILABLE, None),
    };

    let description = error.description();
    let body = format!(
        "{{\"error\":{},\"error_description\":{}}}",
        serde_json::Value::from(oauth_code.unwrap_or("unauthorized")),
        serde_json::Value::from(description),
    );

    let mut response = Response::new(body.into());
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );

    // 503 carries no challenge: the client should retry, not re-authorize.
    if status != StatusCode::SERVICE_UNAVAILABLE {
        let mut params: Vec<(&str, &str)> = vec![("resource_metadata", resource_metadata_url)];
        if let Some(code) = oauth_code {
            params.push(("error", code));
            params.push(("error_description", description));
        }
        let missing_scopes;
        if let McpAuthError::InsufficientScope { missing } = error {
            missing_scopes = missing.join(" ");
            params.push(("scope", &missing_scopes));
        }
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, www_authenticate(&params));
    }
    response
}
