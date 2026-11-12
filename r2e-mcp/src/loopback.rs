//! In-process (loopback) transport: open MCP sessions on the mounted
//! endpoint without the network stack (feature `loopback`).
//!
//! The endpoint is the [`McpServer`](crate::McpServer) one — same `#[routes]`
//! type, same catalog, same [`McpSessions`](crate::McpSessions) — reached
//! through a second inbound transport instead of Streamable HTTP. A loopback
//! session is a real session: it appears in `McpSessions`, its id is
//! [`ToolCall::session_id`](crate::ToolCall::session_id), the
//! [`McpSessionInit`](crate::McpSessionInit) hook runs for it, guards, roles
//! and scopes apply. What differs is *where the identity comes from*: it is
//! resolved once, at [`connect`](McpLoopback::connect), from a
//! [`LoopbackIdentity`], into the same request parts the HTTP layers would
//! have produced — the handler path does not know which transport it serves.
//!
//! ```rust,ignore
//! use r2e_mcp::loopback::{LoopbackIdentity, McpLoopback};
//!
//! let session = loopback
//!     .connect("agent-42", LoopbackIdentity::Bearer(token))
//!     .await?;
//! let tools = session.list_tools(None).await?;
//! session.close().await;
//! ```
//!
//! Loopback ids never reach the HTTP transport's session table: to HTTP a
//! loopback id is an unknown id (`404 Session not found`) once the request
//! gets as far as the session lookup — `mcp.auth`, request scopes and the
//! transport's own request validation (`Accept`, `Content-Type`) still answer
//! first.

use std::fmt;
use std::ops::Deref;
use std::sync::{Arc, OnceLock, Weak};

use futures_channel::mpsc;
use futures_util::StreamExt;
use r2e_core::http::body::to_bytes;
use r2e_core::http::{Body, HeaderValue, Method, Parts, Request, StatusCode, AUTHORIZATION};
use r2e_core::rt::{self, CancelToken, JobHandle};
use r2e_core::Late;
use rmcp::model::{ClientJsonRpcMessage, GetExtensions, JsonRpcMessage, ServerJsonRpcMessage};
use rmcp::service::{ClientInitializeError, RunningService};
use rmcp::transport::sink_stream::SinkStreamTransport;
use rmcp::{ClientHandler, RoleClient, ServiceExt};

/// The `tools/call` request a [`LoopbackSession`] sends
/// (`CallToolRequestParams::new("add").with_arguments(..)`), re-exported so
/// callers need no direct `rmcp` dependency.
pub use rmcp::model::CallToolRequestParams;

use crate::auth::error::McpAuthError;
use crate::auth::layer::{check_required_scopes, deposit_principal};
use crate::auth::{McpPrincipal, McpTokenValidator};
use crate::handler::{Endpoint, R2eMcpHandler};
use crate::identity::{rejected, run_scopes, IdentityState};
use crate::session::{SessionBindings, SessionId, SessionState, SESSION_ID_HEADER};

/// Largest rejection body kept in [`LoopbackError::Rejected`].
const REJECTION_BODY_LIMIT: usize = 16 * 1024;

/// Who opens a loopback session.
///
/// Mirrors what the HTTP transport learns from a request: nothing, a bearer
/// token it validates, or — in-process only — a principal the caller has
/// already established (an agent loop running on behalf of a signed-in
/// user).
#[derive(Clone, Debug)]
pub enum LoopbackIdentity {
    /// No credentials. Refused with
    /// [`LoopbackError::Unauthorized`]`(`[`McpAuthError::MissingToken`]`)`
    /// when `mcp.auth` is on — the loopback is as fail-closed as the HTTP
    /// layer.
    Anonymous,
    /// A bearer token (the raw value, without `Bearer `), validated by the
    /// endpoint's [`McpTokenValidator`] (pinned validators included) and
    /// checked against `mcp.auth.required-scopes`, exactly like an HTTP
    /// request.
    Bearer(String),
    /// A principal the caller vouches for. Deposited as the request identity
    /// without validation; `mcp.auth.required-scopes` still applies to its
    /// scopes. Only reachable from inside the process by construction.
    Principal(McpPrincipal),
}

/// Why a [`connect`](McpLoopback::connect) did not open a session.
#[derive(Debug)]
pub enum LoopbackError {
    /// The MCP endpoint is not mounted: `mcp.enabled = false`, the app has
    /// not finished building its routes yet, or the router was dropped.
    NotMounted,
    /// `mcp.stateless = true`: the endpoint keeps no sessions to open.
    Stateless,
    /// The session id is empty or not header-safe (visible ASCII, no
    /// whitespace, `,` or `;`).
    InvalidSessionId(String),
    /// A live session already carries this id; close it first.
    SessionInUse(SessionId),
    /// The bearer token was refused, lacks a required scope, or `mcp.auth`
    /// is on and no credentials were given.
    Unauthorized(McpAuthError),
    /// A service's struct-level request scope (`#[inject(identity)]`,
    /// `#[inject(request)]`) rejected the synthesized request: the HTTP
    /// status and body it would have answered with.
    Rejected(StatusCode, String),
    /// The MCP `initialize` handshake failed.
    Handshake(Box<ClientInitializeError>),
}

impl fmt::Display for LoopbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotMounted => f.write_str("MCP endpoint not mounted"),
            Self::Stateless => f.write_str("MCP endpoint is stateless (mcp.stateless = true)"),
            Self::InvalidSessionId(id) => write!(f, "invalid MCP session id {id:?}"),
            Self::SessionInUse(id) => write!(f, "MCP session {id} is already open"),
            Self::Unauthorized(err) => write!(f, "loopback connect unauthorized: {err}"),
            Self::Rejected(status, body) => {
                write!(f, "loopback connect rejected ({status}): {body}")
            }
            Self::Handshake(err) => write!(f, "MCP initialize failed: {err}"),
        }
    }
}

impl std::error::Error for LoopbackError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unauthorized(err) => Some(err),
            Self::Handshake(err) => Some(err.as_ref()),
            _ => None,
        }
    }
}

/// The `mcp.auth` inputs a bearer connect validates against: the validator
/// slot the HTTP layer reads (filled from the bean context after the graph
/// resolves) and the server-wide required scopes.
pub(crate) struct LoopbackAuth {
    pub(crate) slot: Arc<OnceLock<McpTokenValidator>>,
    pub(crate) required_scopes: Arc<[String]>,
    /// The canonical resource URI (`mcp.auth.resource`, or what it derives
    /// from); `None` on the Basic-only surface. Read by the test client to
    /// mint tokens bound to this endpoint.
    #[cfg_attr(not(feature = "testing"), allow(dead_code))]
    pub(crate) resource: Option<String>,
}

/// Everything a connect needs, filled by the plugin once the endpoint is
/// mounted (`after_routes`).
pub(crate) struct LoopbackWiring {
    pub(crate) endpoint: Arc<Endpoint>,
    /// The mount path, so the synthesized request matches what the
    /// services' request scopes would see over HTTP.
    pub(crate) path: String,
    pub(crate) stateless: bool,
    /// The services' struct-level request scopes (`None`: no service
    /// declares any).
    pub(crate) identity: Option<Arc<IdentityState>>,
    /// `None` when `mcp.auth` is off.
    pub(crate) auth: Option<LoopbackAuth>,
    pub(crate) bindings: SessionBindings,
    /// The transport-wide shutdown token: ends every loopback session with
    /// the HTTP ones.
    pub(crate) shutdown: CancelToken,
}

/// Open in-process MCP sessions on the [`McpServer`](crate::McpServer)
/// endpoint. A bean (`#[inject] loopback: McpLoopback`), provided by the
/// plugin under feature `loopback`.
///
/// The bean is a handle: the router owns the wiring (the services' request
/// scopes capture the app state, and the bean lives inside that state), so
/// the bean only holds a weak reference and the mounted endpoint goes away
/// with the router instead of leaking the state through a cycle.
#[derive(Clone, Default)]
pub struct McpLoopback {
    wiring: Late<Weak<LoopbackWiring>>,
}

impl McpLoopback {
    /// The bean before the endpoint is mounted: every connect is
    /// [`LoopbackError::NotMounted`] until [`mount`](Self::mount) runs.
    pub(crate) fn unmounted() -> Self {
        Self::default()
    }

    /// Mount the endpoint. Returns the owning handle: the caller (the HTTP
    /// service the router holds) keeps it alive; the bean only observes it.
    pub(crate) fn mount(&self, wiring: LoopbackWiring) -> Arc<LoopbackWiring> {
        let wiring = Arc::new(wiring);
        if self.wiring.fill(Arc::downgrade(&wiring)).is_err() {
            tracing::warn!("MCP loopback wired twice; keeping the first endpoint");
        }
        wiring
    }

    fn mounted(&self) -> Option<Arc<LoopbackWiring>> {
        self.wiring.get().and_then(Weak::upgrade)
    }

    /// The resource URI the endpoint's tokens are bound to, when `mcp.auth`
    /// validates tokens.
    #[cfg(feature = "testing")]
    pub(crate) fn auth_resource(&self) -> Option<String> {
        self.mounted()?.auth.as_ref()?.resource.clone()
    }

    /// Whether the endpoint is mounted (connects can succeed).
    pub fn is_mounted(&self) -> bool {
        self.mounted().is_some()
    }

    /// Open a session named `session_id` as `identity`.
    ///
    /// The identity is resolved once, here; the session then serves every
    /// message under it. The id is the one
    /// [`ToolCall::session_id`](crate::ToolCall::session_id) and
    /// [`McpSessions::get`](crate::McpSessions::get) report; a live session
    /// already holding it is [`LoopbackError::SessionInUse`].
    pub async fn connect(
        &self,
        session_id: impl Into<String>,
        identity: LoopbackIdentity,
    ) -> Result<LoopbackSession, LoopbackError> {
        self.connect_with(session_id, identity, ()).await
    }

    /// [`connect`](Self::connect) with the rmcp client handler serving the
    /// client side: what receives the server's notifications and requests
    /// (`()` ignores the former and refuses the latter).
    pub(crate) async fn connect_with<H: ClientHandler>(
        &self,
        session_id: impl Into<String>,
        identity: LoopbackIdentity,
        client: H,
    ) -> Result<LoopbackSession<H>, LoopbackError> {
        let wiring = self.mounted().ok_or(LoopbackError::NotMounted)?;
        if wiring.stateless {
            return Err(LoopbackError::Stateless);
        }
        let session_id = session_id.into();
        let header = valid_session_id(&session_id)
            .ok_or_else(|| LoopbackError::InvalidSessionId(session_id.clone()))?;

        // The request the HTTP layers would have seen: POST on the mount
        // path, carrying the session id and, for a bearer, the credentials —
        // a scope reading the `Authorization` header finds it there too.
        let (mut parts, _) = Request::new(Body::empty()).into_parts();
        parts.method = Method::POST;
        parts.uri = wiring.path.parse().map_err(|_| LoopbackError::NotMounted)?;
        parts.headers.insert(SESSION_ID_HEADER, header);
        resolve_identity(&wiring, identity, &mut parts).await?;
        if let Some(identity) = &wiring.identity {
            let caller = run_scopes(&identity.scopes, &mut parts)
                .await
                .map_err(|rejection| rejected(rejection, identity.challenges.as_ref()));
            match caller {
                Ok(Some(caller)) => {
                    parts.extensions.insert(caller);
                }
                Ok(None) => {}
                Err(response) => {
                    let (head, body) = response.into_parts();
                    let bytes = to_bytes(body, REJECTION_BODY_LIMIT)
                        .await
                        .unwrap_or_default();
                    return Err(LoopbackError::Rejected(
                        head.status,
                        String::from_utf8_lossy(&bytes).into_owned(),
                    ));
                }
            }
        }

        // The session, named and claimed before its first message so a
        // second connect with the same id is refused atomically.
        let handler = R2eMcpHandler::new(Arc::clone(&wiring.endpoint));
        handler.state().set_id(&session_id);
        let id = handler
            .state()
            .id()
            .cloned()
            .expect("session id set just above");
        if !wiring.endpoint.sessions.claim(&session_id, handler.state()) {
            return Err(LoopbackError::SessionInUse(id));
        }
        wiring
            .bindings
            .insert(&session_id, Arc::downgrade(handler.state()));
        // From here on the id is taken: whatever ends this connect short of
        // a `LoopbackSession` (the caller dropping the future mid-handshake,
        // a failed `initialize`) releases it through the guard.
        let mut pending = Pending {
            state: Arc::clone(handler.state()),
            cancel: wiring.shutdown.child_token(),
            server: None,
            armed: true,
        };

        // Two in-memory channels; every message reaching the server carries
        // the resolved parts in its extensions — rmcp copies them into the
        // `RequestContext`, where the handler reads them like an HTTP
        // request's.
        let (to_server_tx, to_server_rx) = mpsc::unbounded::<ClientJsonRpcMessage>();
        let (to_client_tx, to_client_rx) = mpsc::unbounded::<ServerJsonRpcMessage>();
        let server_stream = to_server_rx.map(move |mut message| {
            match &mut message {
                JsonRpcMessage::Request(request) => {
                    request.request.extensions_mut().insert(parts.clone());
                }
                JsonRpcMessage::Notification(notification) => {
                    notification
                        .notification
                        .extensions_mut()
                        .insert(parts.clone());
                }
                _ => {}
            }
            message
        });
        let server_transport = SinkStreamTransport::new(to_client_tx, server_stream);
        let client_transport = SinkStreamTransport::new(to_server_tx, to_client_rx);

        let server_cancel = pending.cancel.clone();
        let server_state = Arc::clone(handler.state());
        pending.server = Some(rt::spawn(async move {
            // Whatever ends this task — the serve loop returning, the token
            // (close / app shutdown), an abort — the connection is over:
            // the state is closed so the registries forget the id.
            let _closed = CloseOnDrop(server_state);
            // The token ends the serve loop (and the handshake wait) the
            // moment the app shuts down or the session closes.
            match handler
                .serve_with_ct(server_transport, server_cancel.into())
                .await
            {
                Ok(running) => {
                    let _ = running.waiting().await;
                }
                Err(err) => {
                    tracing::debug!(error = %err, "MCP loopback server side ended at initialize");
                }
            }
        }));
        match client.serve(client_transport).await {
            Ok(client) => {
                let (state, cancel, server) = pending.into_session_parts();
                Ok(LoopbackSession {
                    id,
                    client: Some(client),
                    state,
                    cancel,
                    server,
                })
            }
            // `pending` drops here: cancel + abort + close.
            Err(err) => Err(LoopbackError::Handshake(Box::new(err))),
        }
    }
}

/// Marks a session state closed when dropped — the last thing the server
/// task does, on every exit path.
struct CloseOnDrop(Arc<SessionState>);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// A connect between claiming its id and handing out the
/// [`LoopbackSession`]: dropped on any other exit (the future cancelled by
/// its caller, a failed handshake), it ends the server side and releases the
/// id. [`into_session_parts`](Self::into_session_parts) disarms it.
struct Pending {
    state: Arc<SessionState>,
    cancel: CancelToken,
    /// `None` until the server task is spawned.
    server: Option<JobHandle<()>>,
    /// Cleared by [`into_session_parts`](Self::into_session_parts).
    armed: bool,
}

impl Pending {
    fn into_session_parts(mut self) -> (Arc<SessionState>, CancelToken, JobHandle<()>) {
        self.armed = false;
        let server = self
            .server
            .take()
            .expect("server task spawned before the handshake");
        (Arc::clone(&self.state), self.cancel.clone(), server)
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.state.close();
        self.cancel.cancel();
        if let Some(server) = self.server.take() {
            server.abort();
        }
    }
}

impl fmt::Debug for McpLoopback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpLoopback")
            .field("mounted", &self.is_mounted())
            .finish()
    }
}

/// A session id is a header value without separators: visible ASCII only,
/// no whitespace, `,` or `;` (the id travels as `Mcp-Session-Id`).
fn valid_session_id(id: &str) -> Option<HeaderValue> {
    if id.is_empty()
        || id
            .bytes()
            .any(|b| !b.is_ascii_graphic() || matches!(b, b',' | b';'))
    {
        return None;
    }
    HeaderValue::from_str(id).ok()
}

/// Resolve the identity into `parts`, the way the `mcp.auth` layer would:
/// a principal (validated or vouched for) is deposited as the request
/// identity; `Anonymous` is refused when auth is on.
async fn resolve_identity(
    wiring: &LoopbackWiring,
    identity: LoopbackIdentity,
    parts: &mut Parts,
) -> Result<(), LoopbackError> {
    let auth = wiring.auth.as_ref();
    let principal = match identity {
        LoopbackIdentity::Anonymous => {
            if auth.is_some() {
                return Err(LoopbackError::Unauthorized(McpAuthError::MissingToken));
            }
            return Ok(());
        }
        LoopbackIdentity::Bearer(token) => {
            // The header travels with or without `mcp.auth`: a service's
            // request scope reading `Authorization` sees what an HTTP
            // request would have carried.
            let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
                LoopbackError::Unauthorized(McpAuthError::InvalidToken(
                    "token is not a valid header value",
                ))
            })?;
            parts.headers.insert(AUTHORIZATION, value);
            let Some(auth) = auth else {
                // Auth off: nothing validates the token, the session is
                // anonymous — the HTTP endpoint ignores the header too.
                return Ok(());
            };
            let validator = auth.slot.get().ok_or(LoopbackError::NotMounted)?;
            validator
                .validate(&token)
                .await
                .map_err(LoopbackError::Unauthorized)?
        }
        LoopbackIdentity::Principal(principal) => principal,
    };
    if let Some(auth) = auth {
        check_required_scopes(&principal, &auth.required_scopes)
            .map_err(LoopbackError::Unauthorized)?;
    }
    deposit_principal(&mut parts.extensions, principal);
    Ok(())
}

/// A live loopback session: the rmcp client side, ready for
/// `list_tools` / `call_tool` / … through `Deref`.
///
/// Dropping it ends the session; [`close`](Self::close) does so gracefully
/// (the server side finishes its drain before this returns).
///
/// `H` is the rmcp client handler on the client side — `()` for every
/// session [`McpLoopback::connect`] opens.
pub struct LoopbackSession<H: ClientHandler = ()> {
    id: SessionId,
    /// `None` only between `close` taking it and `Drop`.
    client: Option<RunningService<RoleClient, H>>,
    /// The server side's session state: closed with the session, so the id
    /// is free again at once even while an `McpSession` handle holds it.
    state: Arc<SessionState>,
    /// Child of the endpoint's shutdown token: ends the server task.
    cancel: CancelToken,
    server: JobHandle<()>,
}

impl<H: ClientHandler> LoopbackSession<H> {
    /// The session id given to [`connect`](McpLoopback::connect).
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    /// Close the session: the client side is cancelled, then the server
    /// side is awaited. The id is claimable again when this returns.
    pub async fn close(mut self) {
        self.state.close();
        if let Some(client) = self.client.take() {
            let _ = client.cancel().await;
        }
        self.cancel.cancel();
        let _ = (&mut self.server).await;
    }
}

impl<H: ClientHandler> Deref for LoopbackSession<H> {
    type Target = RunningService<RoleClient, H>;

    fn deref(&self) -> &Self::Target {
        self.client
            .as_ref()
            .expect("loopback session client taken by close()")
    }
}

impl<H: ClientHandler> Drop for LoopbackSession<H> {
    fn drop(&mut self) {
        // The id is free now, not when the server task gets to run its
        // teardown; the client's own drop guard cancels its side; the token
        // ends the server loop; the abort is the backstop for a task still
        // parked.
        self.state.close();
        self.cancel.cancel();
        self.server.abort();
    }
}

impl<H: ClientHandler> fmt::Debug for LoopbackSession<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoopbackSession")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
