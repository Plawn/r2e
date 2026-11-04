//! The endpoint-level request scope of `#[routes]` services: the
//! struct's `#[inject(identity)]` / `#[inject(request)]` fields, extracted
//! ONCE per HTTP request by a tower layer on the MCP endpoint — the MCP
//! counterpart of the extraction an HTTP handler runs before it binds the
//! controller's request façade.
//!
//! The contract is the HTTP one, unchanged: `#[controller]` generates the
//! request-data type (`__R2eRequestData_<Name>`, one `FromRequestPartsVia`
//! bound per field) and the façade; `#[routes]` hands the layer the
//! `RequestData::extract` call as a [`RequestScope`] closure, and every
//! member binds its façade from the data the layer deposited in the request
//! extensions ([`request_data`]). A required identity on a request that
//! carries none rejects the WHOLE request — `initialize`, `tools/list`,
//! `tools/call`, the SSE `GET` — before any JSON-RPC message is dispatched,
//! exactly like an HTTP route fails closed: the rejection is rendered
//! through its own [`IntoHttpResponse`] (401 for an unauthenticated
//! extractor), with the `mcp.auth` challenges appended when that layer is
//! configured.
//!
//! What the layer learns about the caller is a [`McpCaller`]: the subject
//! (every [`Identity`]) and the roles (a [`RoleBasedIdentity`]), probed at
//! codegen time by [`CallerProbe`] without requiring either trait — so a
//! struct identity that implements neither still works for a service
//! without guards. The caller drives the same two things the `mcp.auth`
//! principal drives: session ↔ subject binding (a session opened by one
//! subject is a 404 for another) and `#[roles]` list filtering.

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use r2e_core::http::header::{self, HeaderValue};
use r2e_core::http::response::IntoResponse;
use r2e_core::http::{IntoHttpResponse, Method, Parts, Request, Response, StatusCode};
use r2e_core::{Identity, Rejection};
use r2e_security::RoleBasedIdentity;
use tower::{Layer, Service};

use crate::auth::error::Challenges;
use crate::auth::McpPrincipal;
use crate::request_log::AuthOutcome;
use crate::session::{SessionBindings, SessionLink, SESSION_ID_HEADER};

/// The subject and roles the request scope of a service reports back,
/// deposited in the request extensions for session binding, pagination
/// cursors and list filtering. Absent when the service's identity is
/// `Option<I>` and the request carries none.
#[derive(Clone, Debug)]
pub struct McpCaller {
    /// [`Identity::sub`].
    pub sub: String,
    /// [`RoleBasedIdentity::roles`]; empty when the identity type carries
    /// none.
    pub roles: Vec<String>,
}

/// A service's once-per-request extraction: `RequestData::extract` over the
/// `#[controller]`-generated request-data type against the application
/// state, depositing the result for the members. `Ok(None)` means the
/// request scope holds no identity (none declared, or an optional one that
/// the request did not carry).
#[derive(Clone)]
pub struct RequestScope(Arc<dyn ScopeFn>);

/// The future one scope run returns: the caller it found, or the
/// extractor's rejection.
pub type ScopeFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<McpCaller>, Rejection>> + Send + 'a>>;

/// The generated extraction closure behind a [`RequestScope`].
pub trait ScopeFn: for<'a> Fn(&'a mut Parts) -> ScopeFuture<'a> + Send + Sync {}

impl<F> ScopeFn for F where F: for<'a> Fn(&'a mut Parts) -> ScopeFuture<'a> + Send + Sync {}

impl RequestScope {
    /// Wrap the generated extraction closure.
    pub fn new<F: ScopeFn + 'static>(extract: F) -> Self {
        RequestScope(Arc::new(extract))
    }

    async fn run(&self, parts: &mut Parts) -> Result<Option<McpCaller>, Rejection> {
        (self.0)(parts).await
    }
}

impl std::fmt::Debug for RequestScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RequestScope")
    }
}

/// Deposit a service's extracted request data for its members (one `Arc`,
/// cloned per member call).
pub fn deposit_request_data<D: Send + Sync + 'static>(parts: &mut Parts, data: D) {
    parts.extensions.insert(Arc::new(data));
}

/// The request data a [`RequestScope`] deposited, for a member call —
/// `None` when the call did not come through the endpoint layer (a
/// hand-built call).
pub fn request_data<D: Clone + Send + Sync + 'static>(parts: Option<&Parts>) -> Option<D> {
    parts?.extensions.get::<Arc<D>>().map(|d| D::clone(d))
}

// ── Caller probe (autoref specialization) ───────────────────────────────────

/// Codegen-time probe of what a service's identity type can tell about the
/// caller: `(&&CallerProbe::<I>::new()).caller(id)` resolves — by method
/// autoref priority — to [`CallerViaRoles`] when `I: RoleBasedIdentity`,
/// else [`CallerViaIdentity`] when `I: Identity`, else [`CallerViaNone`].
/// None of the three bounds is required of the type.
pub struct CallerProbe<I>(PhantomData<I>);

impl<I> CallerProbe<I> {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        CallerProbe(PhantomData)
    }
}

/// Highest priority: subject + roles.
pub trait CallerViaRoles<I> {
    fn caller(&self, identity: Option<&I>) -> Option<McpCaller>;
}

impl<I: RoleBasedIdentity> CallerViaRoles<I> for &CallerProbe<I> {
    fn caller(&self, identity: Option<&I>) -> Option<McpCaller> {
        identity.map(|id| McpCaller {
            sub: id.sub().to_owned(),
            roles: id.roles().to_vec(),
        })
    }
}

/// Middle priority: subject only.
pub trait CallerViaIdentity<I> {
    fn caller(&self, identity: Option<&I>) -> Option<McpCaller>;
}

impl<I: Identity> CallerViaIdentity<I> for &&CallerProbe<I> {
    fn caller(&self, identity: Option<&I>) -> Option<McpCaller> {
        identity.map(|id| McpCaller {
            sub: id.sub().to_owned(),
            roles: Vec::new(),
        })
    }
}

/// Fallback: the identity type says nothing about the caller.
pub trait CallerViaNone<I> {
    fn caller(&self, identity: Option<&I>) -> Option<McpCaller>;
}

impl<I> CallerViaNone<I> for CallerProbe<I> {
    fn caller(&self, _identity: Option<&I>) -> Option<McpCaller> {
        None
    }
}

// ── The caller as the catalog sees it ───────────────────────────────────────

/// Who is calling, for list filtering, session binding and pagination:
/// the `mcp.auth` principal when that layer deposited one, else the subject
/// a service's request scope reported. Scope requirements need the
/// principal (a token); role requirements are met by either.
#[derive(Clone, Copy)]
pub(crate) struct Caller<'a> {
    pub sub: &'a str,
    roles: &'a [String],
    pub principal: Option<&'a McpPrincipal>,
}

impl Caller<'_> {
    pub(crate) fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }
}

/// The caller carried by the HTTP request parts the transport copies into
/// each JSON-RPC message's extensions.
pub(crate) fn caller_in(parts: Option<&Parts>) -> Option<Caller<'_>> {
    let extensions = &parts?.extensions;
    if let Some(principal) = extensions.get::<McpPrincipal>() {
        return Some(Caller {
            sub: &principal.user.sub,
            roles: principal.user.roles(),
            principal: Some(principal),
        });
    }
    extensions.get::<McpCaller>().map(|caller| Caller {
        sub: &caller.sub,
        roles: &caller.roles,
        principal: None,
    })
}

// ── The layer ───────────────────────────────────────────────────────────────

/// Everything the identity service needs per request, built once by the
/// plugin.
pub(crate) struct IdentityState {
    /// One scope per registered service that declares request-scoped
    /// fields; every one must succeed for the request to go through.
    pub scopes: Vec<RequestScope>,
    /// Shared with the `mcp.auth` layer: whichever of the two learns the
    /// subject binds the session.
    pub bindings: SessionBindings,
    /// The `mcp.auth` challenges, appended to a 401 the extractor rendered
    /// without one — so an OAuth client still discovers the IdP.
    pub challenges: Option<Challenges>,
}

/// Tower layer running every registered service's [`RequestScope`] on the
/// MCP endpoint, between the `mcp.auth` layer (which deposits the principal
/// an `McpPrincipal` identity extracts) and the transport.
#[derive(Clone)]
pub(crate) struct McpIdentityLayer {
    state: Arc<IdentityState>,
}

impl McpIdentityLayer {
    pub(crate) fn new(state: IdentityState) -> Self {
        McpIdentityLayer {
            state: Arc::new(state),
        }
    }
}

impl<S> Layer<S> for McpIdentityLayer {
    type Service = McpIdentityService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        McpIdentityService {
            inner,
            state: Arc::clone(&self.state),
        }
    }
}

/// The service produced by [`McpIdentityLayer`].
#[derive(Clone)]
pub(crate) struct McpIdentityService<S> {
    inner: S,
    state: Arc<IdentityState>,
}

/// rmcp's answer to an unknown or terminated session, byte for byte.
fn session_not_found() -> Response {
    let mut response = Response::new("Not Found: Session not found".into());
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

/// Render a scope's rejection: the extractor's own response, plus the
/// `mcp.auth` challenges on a 401 that carries none.
fn rejected(rejection: Rejection, challenges: Option<&Challenges>) -> Response {
    let mut response = rejection.into_http_response();
    if response.status() == StatusCode::UNAUTHORIZED
        && !response.headers().contains_key(header::WWW_AUTHENTICATE)
        && let Some(challenges) = challenges
    {
        for value in challenges.unauthorized_values() {
            response
                .headers_mut()
                .append(header::WWW_AUTHENTICATE, value);
        }
    }
    response
        .extensions_mut()
        .insert(AuthOutcome("identity_rejected"));
    response
}

impl<S> Service<Request> for McpIdentityService<S>
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

    fn call(&mut self, req: Request) -> Self::Future {
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let state = Arc::clone(&self.state);

        // CORS preflight carries no credentials by design.
        if req.method() == Method::OPTIONS {
            return Box::pin(async move { inner.call(req).await.map(IntoResponse::into_response) });
        }

        Box::pin(async move {
            let (mut parts, body) = req.into_parts();
            let mut caller: Option<McpCaller> = None;
            for scope in &state.scopes {
                match scope.run(&mut parts).await {
                    Ok(found) => {
                        if caller.is_none() {
                            caller = found;
                        }
                    }
                    Err(rejection) => {
                        let response = rejected(rejection, state.challenges.as_ref());
                        tracing::debug!(
                            status = response.status().as_u16(),
                            "MCP request rejected by a service's request scope"
                        );
                        return Ok(response);
                    }
                }
            }

            // Session ↔ subject binding, when the `mcp.auth` layer did not
            // already do it for its principal.
            let authenticated = parts.extensions.get::<McpPrincipal>().is_some();
            let mut link = None;
            if let Some(caller) = &caller {
                if !authenticated {
                    match parts.headers.get(SESSION_ID_HEADER) {
                        Some(id) => {
                            let id = id.to_str().unwrap_or_default();
                            if !state.bindings.admits(id, &caller.sub) {
                                tracing::warn!(
                                    caller = %caller.sub,
                                    "MCP session id presented by a different caller; \
                                     request refused"
                                );
                                let mut response = session_not_found();
                                response
                                    .extensions_mut()
                                    .insert(AuthOutcome("session_mismatch"));
                                return Ok(response);
                            }
                        }
                        None => {
                            let fresh = SessionLink::default();
                            parts.extensions.insert(fresh.clone());
                            link = Some(fresh);
                        }
                    }
                }
                parts.extensions.insert(caller.clone());
            }

            let mut response = inner
                .call(Request::from_parts(parts, body))
                .await?
                .into_response();
            if caller.is_some() && !authenticated {
                response.extensions_mut().insert(AuthOutcome("identity"));
            }
            if let Some(opened) = link.and_then(|link| link.take())
                && let Some(id) = response
                    .headers()
                    .get(SESSION_ID_HEADER)
                    .and_then(|v| v.to_str().ok())
            {
                state.bindings.insert(id, opened);
            }
            Ok(response)
        })
    }
}

/// `WWW-Authenticate` values for a plain 401 (no OAuth error code).
impl Challenges {
    pub(crate) fn unauthorized_values(&self) -> Vec<HeaderValue> {
        let mut values = Vec::with_capacity(2);
        if let Some(url) = &self.resource_metadata_url {
            values.push(crate::auth::error::www_authenticate(&[(
                "resource_metadata",
                url,
            )]));
        }
        if let Some(realm) = &self.basic_realm {
            values.push(crate::auth::error::basic_challenge(realm));
        }
        values
    }
}
