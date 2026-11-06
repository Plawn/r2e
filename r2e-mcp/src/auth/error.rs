//! Auth-layer errors and `WWW-Authenticate` challenge building.

use std::sync::Arc;

use r2e_core::http::header::{self, HeaderValue};
use r2e_core::http::{Response, StatusCode};

/// Why a request's credentials were not accepted.
///
/// Rendered by the auth layer as an OAuth-shaped HTTP error (RFC 6750): 401
/// with the configured `WWW-Authenticate` challenge(s) (`Bearer …` and/or
/// `Basic realm=…`), 403 for `insufficient_scope`, 503 when the upstream
/// IdP is unreachable (no challenge — an IdP outage must not send clients
/// into a re-auth loop).
#[derive(Debug, Clone)]
pub enum McpAuthError {
    /// No usable `Authorization` header (401; a Bearer challenge carries
    /// `resource_metadata` so the client can discover the IdP).
    MissingToken,
    /// The token or credentials failed validation (401,
    /// `error="invalid_token"`). The message is a static, allow-listed
    /// reason — never token or credential contents.
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
            McpAuthError::MissingToken => "missing credentials",
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

/// The `cause` of a request that presented no credentials at all — the
/// normal first leg of every OAuth flow, logged at DEBUG by the layer.
pub(crate) const MISSING_CREDENTIALS: &str = "missing_credentials";

/// An auth failure as the layer sees it: the client-facing [`McpAuthError`]
/// plus the operator-facing cause the rejection log line carries.
///
/// Crate-private by design: the wire only ever renders `error` (a static,
/// allow-listed reason); `cause` and `detail` go to the log, never to the
/// client. `detail` is never token or credential bytes — at most the
/// validator's own error text (a `kid`, an algorithm, a claim name).
#[derive(Debug)]
pub(crate) struct AuthFailure {
    pub error: McpAuthError,
    /// Stable snake_case category (`missing_credentials`, `unknown_kid`, …).
    pub cause: &'static str,
    pub detail: Option<String>,
}

impl AuthFailure {
    pub(crate) fn new(error: McpAuthError, cause: &'static str) -> Self {
        Self {
            error,
            cause,
            detail: None,
        }
    }

    pub(crate) fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub(crate) fn with_cause(mut self, cause: &'static str) -> Self {
        self.cause = cause;
        self
    }
}

/// The generic categorization, for errors coming out of a backend that
/// does not classify its own failures (opaque backends, custom validators):
/// their `InvalidToken` reason is already specific and is logged as `reason`.
impl From<McpAuthError> for AuthFailure {
    fn from(error: McpAuthError) -> Self {
        let (cause, detail) = match &error {
            McpAuthError::MissingToken => (MISSING_CREDENTIALS, None),
            McpAuthError::InvalidToken(_) => ("invalid_token", None),
            McpAuthError::InsufficientScope { missing } => {
                ("insufficient_scope", Some(missing.join(" ")))
            }
            McpAuthError::InvalidOrigin => ("invalid_origin", None),
            McpAuthError::Upstream(detail) => ("upstream_unavailable", Some(detail.clone())),
        };
        Self {
            error,
            cause,
            detail,
        }
    }
}

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

/// One challenge of a `WWW-Authenticate` field (RFC 9110 §11.6.1): the
/// scheme and its parameters, each kept verbatim (`name=value`, quoted or
/// not, or a token68) so completing a challenge never rewrites what the
/// extractor chose to advertise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Challenge {
    pub scheme: String,
    pub params: Vec<String>,
}

impl Challenge {
    /// Whether this challenge uses `scheme` (case-insensitive, RFC 9110 §11.1).
    pub(crate) fn scheme_is(&self, scheme: &str) -> bool {
        self.scheme.eq_ignore_ascii_case(scheme)
    }

    /// Whether a parameter named `name` is present (names are
    /// case-insensitive and may be padded around `=`, RFC 9110 §11.2).
    pub(crate) fn has_param(&self, name: &str) -> bool {
        self.params.iter().any(|param| {
            param
                .split_once('=')
                .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case(name))
        })
    }

    /// Append `name="value"` (RFC 9110 quoted-string escaping).
    pub(crate) fn push_quoted(&mut self, name: &str, value: &str) {
        self.params.push(format!("{name}={}", quote(value)));
    }

    /// The challenge as one `WWW-Authenticate` value.
    pub(crate) fn render(&self) -> HeaderValue {
        let text = if self.params.is_empty() {
            self.scheme.clone()
        } else {
            format!("{} {}", self.scheme, self.params.join(", "))
        };
        // Every piece came out of a header value (or `quote`), so the text is
        // visible ASCII; the fallback is unreachable in practice.
        HeaderValue::from_str(&text).unwrap_or_else(|_| HeaderValue::from_static("Bearer"))
    }
}

/// Split a `WWW-Authenticate` value into its challenges.
///
/// Commas inside quoted strings (and `\"` escapes) do not split. A
/// comma-separated item whose first word carries no `=` starts a new
/// challenge (`Bearer realm="x"`, `Negotiate abc==`); any other item is a
/// parameter of the current one. An item that cannot belong to a challenge
/// (a parameter before any scheme) is kept as an opaque challenge so nothing
/// is dropped. `None` when the value is not visible ASCII.
pub(crate) fn parse_challenges(value: &HeaderValue) -> Option<Vec<Challenge>> {
    let text = value.to_str().ok()?;
    let mut challenges: Vec<Challenge> = Vec::new();
    for item in split_unquoted_commas(text) {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let (head, rest) = item
            .split_once(|c: char| c.is_ascii_whitespace())
            .map_or((item, ""), |(head, rest)| (head, rest.trim()));
        if head.contains('=') {
            match challenges.last_mut() {
                Some(current) => current.params.push(item.to_owned()),
                None => challenges.push(Challenge {
                    scheme: item.to_owned(),
                    params: Vec::new(),
                }),
            }
            continue;
        }
        let mut challenge = Challenge {
            scheme: head.to_owned(),
            params: Vec::new(),
        };
        if !rest.is_empty() {
            challenge.params.push(rest.to_owned());
        }
        challenges.push(challenge);
    }
    Some(challenges)
}

/// Split on the commas that sit outside quoted strings.
fn split_unquoted_commas(text: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        match ch {
            _ if escaped => escaped = false,
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                items.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    items.push(&text[start..]);
    items
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

/// Build a `WWW-Authenticate: Basic …` header value (RFC 7617).
pub fn basic_challenge(realm: &str) -> HeaderValue {
    let value = format!("Basic realm={}, charset=\"UTF-8\"", quote(realm));
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("Basic"))
}

/// The challenges a 401/403 advertises, fixed at boot per auth layer.
#[derive(Clone)]
pub(crate) struct Challenges {
    /// `Bearer resource_metadata="…"` — set when OAuth is configured.
    pub resource_metadata_url: Option<Arc<str>>,
    /// `Basic realm="…"` — set when `mcp.auth.basic` is configured.
    pub basic_realm: Option<Arc<str>>,
}

/// Render an auth failure as the OAuth-shaped HTTP response.
///
/// The Bearer challenge carries `resource_metadata` plus the per-error
/// `error`/`error_description`/`scope` parameters; the Basic challenge is a
/// second `WWW-Authenticate` value (RFC 9110 §11.6.1 allows several), on
/// 401 only — a 403 is not an invitation to retry with other credentials.
pub(crate) fn auth_error_response(error: &McpAuthError, challenges: &Challenges) -> Response {
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
    if status == StatusCode::SERVICE_UNAVAILABLE {
        return response;
    }
    if let Some(resource_metadata_url) = &challenges.resource_metadata_url {
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
            .append(header::WWW_AUTHENTICATE, www_authenticate(&params));
    }
    if let Some(realm) = &challenges.basic_realm
        && status == StatusCode::UNAUTHORIZED
    {
        response
            .headers_mut()
            .append(header::WWW_AUTHENTICATE, basic_challenge(realm));
    }
    response
}
