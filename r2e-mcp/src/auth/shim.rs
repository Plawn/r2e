//! Static Dynamic Client Registration shim + authorization-server metadata
//! mirror.
//!
//! MCP clients (Claude, the Inspector) expect RFC 7591 dynamic registration,
//! but most IdPs ship with anonymous DCR disabled (Keycloak's Trusted Hosts
//! policy) or absent (Google, Entra). The shim answers the registration call
//! itself with a FIXED, pre-registered public client id
//! (`mcp.auth.public-client-id`) — it registers NOTHING on the IdP; the
//! redirect URIs must already be configured on that client.
//!
//! For the client to find this registration endpoint, the shim also mirrors
//! the IdP's discovered metadata (verbatim, from
//! [`DiscoveryClient`](super::DiscoveryClient)) with `registration_endpoint`
//! rewritten to the shim and PKCE/public-client capabilities advertised.
//! **`issuer` is deliberately KEPT as the real IdP's** — rewriting it would
//! break `token_endpoint` use and `iss` validation. That RFC 8414 §3.3
//! tension (document served off-issuer) is tolerated by mainstream MCP
//! clients; `mcp.auth.shim: false` is the escape hatch.

use std::sync::Arc;

use r2e_core::http::response::IntoResponse;
use r2e_core::http::routing::{get, post};
use r2e_core::http::{body::to_bytes, header, HeaderValue, Request, Response, Router, StatusCode};
use serde_json::{json, Value};

use super::discovery::DiscoveryClient;
use super::wellknown::{preflight, public_json_response, put_public_headers};

/// Registration request bodies above this size are rejected (the legitimate
/// payload is a handful of redirect URIs).
const MAX_REGISTER_BODY_BYTES: usize = 16 * 1024;

/// Default redirect-URI allowlist: local development plus the callbacks of
/// the mainstream MCP clients.
pub(crate) const DEFAULT_REDIRECT_ALLOWLIST: &[&str] = &[
    "http://localhost:*",
    "http://127.0.0.1:*",
    "https://claude.ai/api/mcp/auth_callback",
    "https://claude.com/api/mcp/auth_callback",
    "https://inspector.modelcontextprotocol.io/*",
];

/// Everything the shim handlers need, prebuilt at plugin build time.
pub(crate) struct ShimState {
    pub discovery: Arc<DiscoveryClient>,
    /// The pre-registered public client id every "registration" answers with.
    pub client_id: String,
    /// Absolute URL of the shim's own registration endpoint (rewritten into
    /// the mirrored metadata).
    pub registration_endpoint: String,
    /// Extra scopes to union into the mirrored `scopes_supported`.
    pub scopes_supported: Vec<String>,
    /// `mcp.auth.redirect-uri-allowlist`, or the default list.
    pub redirect_allowlist: Vec<String>,
}

/// Match one redirect URI against an allowlist entry: exact match, a
/// trailing `*` prefix wildcard, or `:*` = "any port (then any path)".
fn redirect_allowed(allowed: &[String], uri: &str) -> bool {
    allowed.iter().any(|entry| {
        if let Some(prefix) = entry.strip_suffix(":*") {
            uri.strip_prefix(prefix)
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some_and(|rest| {
                    let digits = rest.chars().take_while(char::is_ascii_digit).count();
                    digits > 0 && matches!(rest[digits..].chars().next(), None | Some('/'))
                })
        } else if let Some(prefix) = entry.strip_suffix('*') {
            uri.starts_with(prefix)
        } else {
            entry == uri
        }
    })
}

fn oauth_error(status: StatusCode, code: &str, description: &str) -> Response {
    let body = json!({ "error": code, "error_description": description }).to_string();
    let mut response = Response::new(body.into());
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    put_public_headers(&mut response);
    response
}

/// `POST {mcp.path}{registration-path}` — the static registration answer.
async fn register(state: Arc<ShimState>, req: Request) -> Response {
    let body = match to_bytes(req.into_body(), MAX_REGISTER_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return oauth_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "invalid_client_metadata",
                "registration request body too large",
            )
        }
    };
    let request: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_client_metadata",
                "registration request is not a JSON object",
            )
        }
    };

    let requested: Vec<&str> = request
        .get("redirect_uris")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let accepted: Vec<&str> = requested
        .iter()
        .copied()
        .filter(|uri| redirect_allowed(&state.redirect_allowlist, uri))
        .collect();
    if accepted.is_empty() {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            "no requested redirect_uri is on the allowlist \
             (`mcp.auth.redirect-uri-allowlist`)",
        );
    }
    if accepted.len() < requested.len() {
        tracing::warn!(
            requested = requested.len(),
            accepted = accepted.len(),
            "DCR shim dropped redirect URIs not on the allowlist"
        );
    }

    let mut doc = json!({
        "client_id": state.client_id,
        "token_endpoint_auth_method": "none",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "redirect_uris": accepted,
    });
    if let Some(name) = request.get("client_name").and_then(Value::as_str) {
        doc["client_name"] = json!(name);
    }

    let mut response = Response::new(doc.to_string().into());
    *response.status_mut() = StatusCode::CREATED;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    put_public_headers(&mut response);
    response
}

/// Mirror the IdP's metadata with the shim's rewrites applied.
fn mirrored_metadata(state: &ShimState, raw: &Value) -> String {
    let mut doc = raw.clone();
    doc["registration_endpoint"] = json!(state.registration_endpoint);

    // PKCE: MCP clients require S256; most IdPs support it but not all
    // advertise it.
    let methods = doc
        .get("code_challenge_methods_supported")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !methods.iter().any(|m| m.as_str() == Some("S256")) {
        let mut methods = methods;
        methods.push(json!("S256"));
        doc["code_challenge_methods_supported"] = Value::Array(methods);
    }

    // The shimmed client is public: "none" must be an advertised auth method.
    let mut auth_methods = doc
        .get("token_endpoint_auth_methods_supported")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !auth_methods.iter().any(|m| m.as_str() == Some("none")) {
        auth_methods.push(json!("none"));
    }
    doc["token_endpoint_auth_methods_supported"] = Value::Array(auth_methods);

    if !state.scopes_supported.is_empty() {
        let mut scopes = doc
            .get("scopes_supported")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for scope in &state.scopes_supported {
            if !scopes.iter().any(|s| s.as_str() == Some(scope)) {
                scopes.push(json!(scope));
            }
        }
        doc["scopes_supported"] = Value::Array(scopes);
    }

    doc.to_string()
}

/// `GET` handler for the mirrored authorization-server metadata routes.
async fn serve_metadata(state: Arc<ShimState>) -> Response {
    match state.discovery.get().await {
        Ok(meta) => public_json_response(&Arc::from(mirrored_metadata(&state, &meta.raw))),
        Err(err) => {
            tracing::warn!(error = %err.description(), "shim metadata mirror: discovery failed");
            oauth_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "temporarily_unavailable",
                "authorization server metadata is currently unavailable",
            )
        }
    }
}

/// The shim router: mirrored metadata (4 paths) + the registration endpoint.
pub(crate) fn shim_routes(
    state: Arc<ShimState>,
    mcp_path: &str,
    registration_path: &str,
) -> Router {
    let meta = {
        let state = state.clone();
        move || {
            let state = state.clone();
            async move { serve_metadata(state).await.into_response() }
        }
    };
    let reg = move |req: Request| {
        let state = state.clone();
        async move { register(state, req).await.into_response() }
    };

    Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            get(meta.clone()).options(preflight),
        )
        .route(
            &format!("/.well-known/oauth-authorization-server{mcp_path}"),
            get(meta.clone()).options(preflight),
        )
        .route(
            "/.well-known/openid-configuration",
            get(meta.clone()).options(preflight),
        )
        .route(
            &format!("{mcp_path}/.well-known/openid-configuration"),
            get(meta).options(preflight),
        )
        .route(
            &format!("{mcp_path}{registration_path}"),
            post(reg).options(preflight),
        )
}
