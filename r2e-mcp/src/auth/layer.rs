//! The tower layer enforcing OAuth on the MCP endpoint.
//!
//! Modelled on `r2e-prometheus/src/layer.rs`, but with a BOXED response
//! future: token validation is async and must complete *before* the inner
//! service is called (Prometheus only observes around the call and can stay
//! allocation-free). MCP traffic is a few long-lived requests, not a hot
//! request path — the box is irrelevant there.
//!
//! Mounted only around the MCP service itself (inside the plugin's
//! `wrap_router` closure); the well-known routes are merged NEXT TO the
//! service and never pass through here.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};

use r2e_core::http::response::IntoResponse;
use r2e_core::http::{HeaderMap, Method, Request, Response, AUTHORIZATION};
use tower::{Layer, Service};

use super::error::{auth_error_response, McpAuthError};
use super::validator::{McpPrincipal, McpTokenValidator};

/// Everything the auth service needs per request, prebuilt once at plugin
/// build time.
pub(crate) struct AuthState {
    /// Filled by the plugin's `after_build` from the bean context, so a
    /// test-pinned validator (`override_bean`) is RESOLVED, not captured.
    pub validator: Arc<OnceLock<McpTokenValidator>>,
    /// Absolute URL of the protected-resource-metadata document, carried in
    /// every challenge (`WWW-Authenticate: Bearer resource_metadata="…"`).
    pub resource_metadata_url: Arc<str>,
    /// Server-wide `mcp.auth.required-scopes` — enforced at HTTP level.
    pub required_scopes: Arc<[String]>,
    /// `mcp.auth.allowed-origins` — when set, a present `Origin` header must
    /// match (DNS-rebinding guard); requests without `Origin` (non-browser
    /// clients) pass.
    pub allowed_origins: Option<Arc<[String]>>,
}

/// Tower layer enforcing `mcp.auth.*` on the MCP endpoint.
#[derive(Clone)]
pub struct McpAuthLayer {
    /// `None` = auth disabled (no `mcp.auth` section): plain pass-through.
    state: Option<Arc<AuthState>>,
}

impl McpAuthLayer {
    /// The pass-through layer used when `mcp.auth` is absent or disabled.
    pub fn disabled() -> Self {
        Self { state: None }
    }

    pub(crate) fn enabled(state: AuthState) -> Self {
        Self {
            state: Some(Arc::new(state)),
        }
    }
}

impl<S> Layer<S> for McpAuthLayer {
    type Service = McpAuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        McpAuthService {
            inner,
            state: self.state.clone(),
        }
    }
}

/// The service produced by [`McpAuthLayer`].
#[derive(Clone)]
pub struct McpAuthService<S> {
    inner: S,
    state: Option<Arc<AuthState>>,
}

/// Origin matching: exact match, or a `:*` suffix wildcard matching any port
/// (`http://localhost:*`).
pub(crate) fn origin_allowed(allowed: &[String], origin: &str) -> bool {
    allowed.iter().any(|entry| {
        if let Some(prefix) = entry.strip_suffix(":*") {
            origin
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with(':') && rest[1..].chars().all(|c| c.is_ascii_digit()))
        } else {
            entry == origin
        }
    })
}

/// Run the pre-call checks; `Ok` carries what to insert into extensions.
async fn authorize(
    state: &AuthState,
    headers: &HeaderMap,
) -> Result<McpPrincipal, McpAuthError> {
    // Origin allowlist (browser clients only — no header, no check).
    if let Some(allowed) = &state.allowed_origins {
        if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
            if !origin_allowed(allowed, origin) {
                return Err(McpAuthError::InvalidOrigin);
            }
        }
    }

    let bearer = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(McpAuthError::MissingToken)
        .and_then(|header| {
            r2e_security::extractor::extract_bearer_token(header)
                .map_err(|_| McpAuthError::MissingToken)
        })?;

    let Some(validator) = state.validator.get() else {
        // after_build has not run — a wiring bug, never a client error.
        tracing::error!("MCP auth validator not initialised (after_build did not run)");
        return Err(McpAuthError::Upstream(
            "authentication not initialised".into(),
        ));
    };

    let principal = validator.validate(bearer).await?;

    let missing: Vec<String> = state
        .required_scopes
        .iter()
        .filter(|s| !principal.has_scope(s))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(McpAuthError::InsufficientScope { missing });
    }

    Ok(principal)
}

impl<S> Service<Request> for McpAuthService<S>
where
    S: Service<Request> + Clone + Send + 'static,
    S::Response: IntoResponse,
    S::Future: Send,
{
    type Response = Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request) -> Self::Future {
        // Standard tower clone dance: the clone made in `poll_ready`-ready
        // state is the one that serves this call.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        let Some(state) = self.state.clone() else {
            return Box::pin(async move { inner.call(req).await.map(IntoResponse::into_response) });
        };

        // CORS preflight carries no Authorization header by design.
        if req.method() == Method::OPTIONS {
            return Box::pin(async move { inner.call(req).await.map(IntoResponse::into_response) });
        }

        Box::pin(async move {
            // Only the headers cross the await — `Request`'s body is not
            // `Sync`, so borrowing the whole request here would un-`Send`
            // the boxed future.
            match authorize(&state, req.headers()).await {
                Ok(principal) => {
                    // rmcp copies `http::request::Parts` extensions into each
                    // JSON-RPC message's `RequestContext.extensions`, so tools
                    // read these via `ToolCall.parts` / identity params.
                    req.extensions_mut().insert(principal.user.clone());
                    req.extensions_mut().insert(principal);
                    inner.call(req).await.map(IntoResponse::into_response)
                }
                Err(err) => Ok(auth_error_response(&err, &state.resource_metadata_url)),
            }
        })
    }
}
