//! RFC 9728 Protected Resource Metadata routes.
//!
//! The PRM document is what turns a bare `401` into a working OAuth flow:
//! the client reads `resource_metadata` from the challenge, fetches this
//! document, and learns which authorization server to talk to. Served at
//! BOTH RFC 9728 forms:
//!
//! - `/.well-known/oauth-protected-resource` (origin-wide), and
//! - `/.well-known/oauth-protected-resource{mcp.path}` (path-suffixed — what
//!   spec-following clients derive from the resource URI).
//!
//! Unauthenticated by design (merged NEXT TO the MCP service, never behind
//! [`McpAuthLayer`](super::McpAuthLayer)). Handlers carry their own
//! permissive CORS headers: the document is public and browser-based clients
//! (claude.ai) fetch it cross-origin before any auth exists.

use std::sync::Arc;

use r2e_core::http::response::IntoResponse;
use r2e_core::http::routing::get;
use r2e_core::http::{header, HeaderValue, Response, Router, StatusCode};
use serde_json::json;

/// Build the pre-serialised PRM document.
///
/// `authorization_servers` is `[issuer]` normally, `[server.public-url]`
/// when the DCR shim is on (clients must then discover THROUGH the shim's
/// mirrored metadata to see the rewritten `registration_endpoint`).
pub(crate) fn prm_json(
    resource: &str,
    authorization_servers: &[String],
    scopes_supported: &[String],
    resource_name: Option<&str>,
) -> Arc<str> {
    let mut doc = json!({
        "resource": resource,
        "authorization_servers": authorization_servers,
        "bearer_methods_supported": ["header"],
    });
    if !scopes_supported.is_empty() {
        doc["scopes_supported"] = json!(scopes_supported);
    }
    if let Some(name) = resource_name {
        doc["resource_name"] = json!(name);
    }
    doc.to_string().into()
}

/// Shared response builder for the public well-known documents: JSON,
/// cacheable, permissive CORS (see module docs), `WWW-Authenticate` and
/// `Mcp-Session-Id` exposed so browser clients can read them.
pub(crate) fn public_json_response(json: &Arc<str>) -> Response {
    let mut response = Response::new(json.to_string().into());
    put_public_headers(&mut response);
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    response
}

/// The CORS headers shared by the well-known/shim handlers and their
/// preflight responses.
pub(crate) fn put_public_headers(response: &mut Response) {
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("authorization, content-type, mcp-protocol-version"),
    );
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("www-authenticate, mcp-session-id"),
    );
}

/// A `204` preflight response with the public CORS headers.
pub(crate) async fn preflight() -> Response {
    let mut response = Response::new(Default::default());
    *response.status_mut() = StatusCode::NO_CONTENT;
    put_public_headers(&mut response);
    response
}

/// The PRM router: both well-known paths, GET + preflight.
pub(crate) fn prm_routes(prm: Arc<str>, mcp_path: &str) -> Router {
    let serve = move || {
        let prm = prm.clone();
        async move { public_json_response(&prm).into_response() }
    };
    let root = "/.well-known/oauth-protected-resource".to_string();
    let suffixed = format!("{root}{mcp_path}");
    Router::new()
        .route(&root, get(serve.clone()).options(preflight))
        .route(&suffixed, get(serve).options(preflight))
}
