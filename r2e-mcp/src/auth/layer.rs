//! The tower layer enforcing `mcp.auth.*` (OAuth bearer and/or HTTP Basic)
//! on the MCP endpoint.
//!
//! Modelled on `r2e-prometheus/src/layer.rs`, but with a BOXED response
//! future: token validation is async and must complete *before* the inner
//! service is called (Prometheus only observes around the call and can stay
//! allocation-free). MCP traffic is a few long-lived requests, not a hot
//! request path — the box is irrelevant there.
//!
//! Mounted only around the MCP service itself (inside the plugin's
//! Routes-stage `after_routes` closure); the well-known routes are merged
//! NEXT TO the service and never pass through here.
//!
//! It also owns the session ↔ principal binding: an `initialize` that opens
//! a session records it under the `Mcp-Session-Id` of the response, and any
//! later request presenting that id — POST, the standalone SSE `GET`,
//! `DELETE` — from another subject gets rmcp's own 404 "session not found"
//! (no oracle that the session exists).

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};

use r2e_core::http::layers::origin_allowed;
use r2e_core::http::response::IntoResponse;
use r2e_core::http::{
    Extensions, HeaderMap, Method, Request, Response, StatusCode, AUTHORIZATION,
};
use tower::{Layer, Service};

use super::basic::{basic_credentials, BasicCredentials};
use super::error::{
    auth_error_response, AuthFailure, Challenges, McpAuthError, MISSING_CREDENTIALS,
};
use super::validator::{McpPrincipal, McpTokenValidator};
use crate::request_log::AuthOutcome;
use crate::session::{session_not_found, SessionBindings, SessionLink};

/// The streamable-HTTP session header (rmcp's `HEADER_SESSION_ID`).
const SESSION_ID: &str = crate::session::SESSION_ID_HEADER;

/// Everything the auth service needs per request, prebuilt once at plugin
/// build time.
pub(crate) struct AuthState {
    /// Filled by the plugin's `after_build` from the bean context, so a
    /// test-pinned validator (`override_bean`) is RESOLVED, not captured.
    pub validator: Arc<OnceLock<McpTokenValidator>>,
    /// Whether bearer tokens are accepted at all (`mcp.auth.issuer` set).
    /// Off ⇒ `Authorization: Bearer …` is an unsupported scheme and the
    /// validator slot is never read.
    pub oauth: bool,
    /// `mcp.auth.basic` credentials, when configured.
    pub basic: Option<BasicCredentials>,
    /// The `WWW-Authenticate` challenge(s) every 401 carries.
    pub challenges: Challenges,
    /// Server-wide `mcp.auth.required-scopes` — enforced at HTTP level.
    pub required_scopes: Arc<[String]>,
    /// `mcp.allowed-origins` — when set, a present `Origin` header must
    /// match (DNS-rebinding guard); requests without `Origin` (non-browser
    /// clients) pass.
    pub allowed_origins: Option<Arc<[String]>>,
    /// Session id → session, shared by every worker (the layer is built
    /// once, before the router is cloned per worker).
    pub bindings: SessionBindings,
}

/// Tower layer enforcing `mcp.auth.*` on the MCP endpoint.
#[derive(Clone)]
pub struct McpAuthLayer {
    state: Arc<AuthState>,
}

impl McpAuthLayer {
    pub(crate) fn new(state: AuthState) -> Self {
        Self {
            state: Arc::new(state),
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
    state: Arc<AuthState>,
}

/// Run the pre-call checks; `Ok` carries what to insert into extensions.
async fn authorize(state: &AuthState, headers: &HeaderMap) -> Result<McpPrincipal, AuthFailure> {
    // Origin allowlist (browser clients only — no header, no check).
    if let Some(allowed) = &state.allowed_origins
        && let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok())
        && !origin_allowed(allowed, origin)
    {
        return Err(AuthFailure::from(McpAuthError::InvalidOrigin).with_detail(origin));
    }

    let header = match headers.get(AUTHORIZATION) {
        None => return Err(McpAuthError::MissingToken.into()),
        Some(value) => value.to_str().map_err(|_| {
            AuthFailure::new(McpAuthError::MissingToken, "malformed_header")
                .with_detail("Authorization header is not visible ASCII")
        })?,
    };

    // The scheme selects the path: Basic when configured and presented,
    // otherwise the bearer path (its own scheme check keeps a stray `Basic`
    // on an OAuth-only endpoint a 401, as before).
    let principal = match (&state.basic, basic_credentials(header)) {
        (Some(basic), Some(encoded)) => basic
            .authenticate(encoded)
            .map_err(|e| AuthFailure::new(e, "basic_credentials"))?,
        _ if state.oauth => {
            let bearer = r2e_security::extractor::extract_bearer_token(header).map_err(|_| {
                AuthFailure::new(McpAuthError::MissingToken, "malformed_header")
                    .with_detail("Authorization header is not `Bearer <token>`")
            })?;
            let Some(validator) = state.validator.get() else {
                // after_build has not run — a wiring bug, never a client
                // error; `log_rejection` reports it (cause + detail).
                return Err(AuthFailure::new(
                    McpAuthError::Upstream("authentication not initialised".into()),
                    "validator_not_initialised",
                )
                .with_detail("after_build did not run"));
            };
            validator.validate_detailed(bearer).await?
        }
        _ => {
            return Err(AuthFailure::new(
                McpAuthError::InvalidToken("unsupported authorization scheme"),
                "unsupported_scheme",
            ))
        }
    };

    check_required_scopes(&principal, &state.required_scopes)?;

    Ok(principal)
}

/// Server-wide `mcp.auth.required-scopes`: every one must be granted to the
/// principal. Shared by the HTTP layer and the loopback transport so a
/// session opened in-process meets exactly the policy an HTTP one does.
pub(crate) fn check_required_scopes(
    principal: &McpPrincipal,
    required: &[String],
) -> Result<(), McpAuthError> {
    let missing: Vec<String> = required
        .iter()
        .filter(|s| !principal.has_scope(s))
        .cloned()
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(McpAuthError::InsufficientScope { missing })
    }
}

/// Deposit the authenticated principal in the request extensions — what
/// rmcp copies into each JSON-RPC message's `RequestContext.extensions`, so
/// tools read it via `ToolCall.parts` / identity params
/// (`#[inject(identity)] McpPrincipal`, marker `ViaMcpAuth`).
///
/// Two extensions, ONE `AuthenticatedUser`: the identity is deposited as
/// `Arc<AuthenticatedUser>` — a refcount bump, not a second copy of the
/// claims tree — and the principal holds the same `Arc`. Extracting the
/// principal clones `Arc`s; `ToolCall::identity::<T>()` materializes an
/// owned `T` only when a member asks for one by hand.
pub(crate) fn deposit_principal(extensions: &mut Extensions, principal: McpPrincipal) {
    extensions.insert(Arc::clone(&principal.user));
    extensions.insert(principal);
}

/// Log a rejection with its cause — never the token or credentials.
///
/// One rule for the level: a request with **no** `Authorization` header is
/// the first leg of every OAuth flow (the client fetches the challenge), so
/// it is DEBUG; every other rejection — a credential that was presented and
/// refused, or an IdP outage — is WARN.
fn log_rejection(failure: &AuthFailure, status: StatusCode) {
    let reason = failure.error.description();
    if failure.cause == MISSING_CREDENTIALS {
        tracing::debug!(
            cause = failure.cause,
            status = status.as_u16(),
            "MCP request without credentials; answering with the OAuth challenge"
        );
    } else {
        // Debug-escaped: `detail` can carry client-controlled text (a
        // `kid`, an `Origin`), which must not forge log lines. `None`
        // leaves the field out instead of printing `detail=""`.
        let detail = failure.detail.as_deref().map(tracing::field::debug);
        tracing::warn!(
            cause = failure.cause,
            status = status.as_u16(),
            reason,
            detail,
            "MCP request rejected"
        );
    }
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

        let state = self.state.clone();

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
                    let link = match req.headers().get(SESSION_ID) {
                        Some(id) => {
                            let id = id.to_str().unwrap_or_default();
                            if !state.bindings.admits(id, Some(&principal.user.sub)) {
                                tracing::warn!(
                                    caller = %principal.user.sub,
                                    "MCP session id presented by a different principal; \
                                     request refused"
                                );
                                let mut response = session_not_found();
                                response
                                    .extensions_mut()
                                    .insert(AuthOutcome("session_mismatch"));
                                return Ok(response);
                            }
                            None
                        }
                        None => {
                            let link = SessionLink::default();
                            req.extensions_mut().insert(link.clone());
                            Some(link)
                        }
                    };

                    deposit_principal(req.extensions_mut(), principal);
                    let mut response = inner.call(req).await?.into_response();
                    response
                        .extensions_mut()
                        .insert(AuthOutcome("authenticated"));
                    if let Some(opened) = link.and_then(|link| link.take())
                        && let Some(id) = response
                            .headers()
                            .get(SESSION_ID)
                            .and_then(|v| v.to_str().ok())
                    {
                        state.bindings.insert(id, opened);
                    }
                    Ok(response)
                }
                Err(failure) => {
                    let mut response = auth_error_response(&failure.error, &state.challenges);
                    log_rejection(&failure, response.status());
                    response.extensions_mut().insert(AuthOutcome(failure.cause));
                    Ok(response)
                }
            }
        })
    }
}
