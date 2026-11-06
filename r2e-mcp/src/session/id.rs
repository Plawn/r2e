use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};

use r2e_core::http::{Request, Response, StatusCode};
use tower::{Layer, Service};

use super::state::SessionState;
use crate::identity::caller_in_extensions;
use crate::request_log::AuthOutcome;

/// The transport-assigned identifier of an MCP session — the value of the
/// `Mcp-Session-Id` header under streamable HTTP.
///
/// Cheap to clone (a shared string). Read it from a call
/// ([`ToolCall::session_id`](crate::ToolCall::session_id)) or a handle
/// ([`McpSession::id`](super::McpSession::id)), and look a session up with [`McpSessions::get`](super::McpSessions::get).
///
/// Known from the request *after* `initialize` on: the handshake itself
/// is the request that mints the id, so [`SessionInit::session_id`](super::SessionInit::session_id) is
/// `None` for an [`McpSessionInit`](super::McpSessionInit) hook running during `initialize`
/// (the id is attached as soon as rmcp's response leaves the server).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SessionId(Arc<str>);

impl SessionId {
    /// The id as sent on the wire.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SessionId {
    fn from(id: &str) -> Self {
        SessionId(Arc::from(id))
    }
}

impl From<String> for SessionId {
    fn from(id: String) -> Self {
        SessionId(Arc::from(id))
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for SessionId {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for SessionId {
    fn eq(&self, other: &str) -> bool {
        &*self.0 == other
    }
}

impl PartialEq<&str> for SessionId {
    fn eq(&self, other: &&str) -> bool {
        &*self.0 == *other
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, f)
    }
}

/// Deposited by the auth layer (or [`TrackSessionId`]) in the extensions of
/// a request that carries no `Mcp-Session-Id`: if it turns out to be an
/// `initialize` that opens a session, the handler hands the new session back
/// through it, the layer keys it by the id rmcp puts in the response header,
/// and the session store adapter reads its subject when rmcp persists it.
#[derive(Clone, Default)]
pub(crate) struct SessionLink(Arc<Mutex<Option<Weak<SessionState>>>>);

impl SessionLink {
    pub(crate) fn set(&self, state: &Arc<SessionState>) {
        *self.0.lock().expect("MCP session link poisoned") = Some(Arc::downgrade(state));
    }

    pub(crate) fn take(&self) -> Option<Weak<SessionState>> {
        self.0.lock().expect("MCP session link poisoned").take()
    }

    pub(crate) fn peek(&self) -> Option<Arc<SessionState>> {
        self.0
            .lock()
            .expect("MCP session link poisoned")
            .as_ref()
            .and_then(Weak::upgrade)
    }
}

/// The streamable-HTTP session header (rmcp's `HEADER_SESSION_ID`).
pub(crate) const SESSION_ID_HEADER: &str = "mcp-session-id";

/// rmcp's answer to an unknown or terminated session, byte for byte — what
/// the auth and identity layers answer a foreign subject with, so a refused
/// replay is indistinguishable from an unknown id.
pub(crate) fn session_not_found() -> Response {
    let mut response = Response::new("Not Found: Session not found".into());
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

/// What the session store adapter needs to know about the HTTP request
/// rmcp is serving when it calls `load` / `store` / `delete`: the caller
/// (owner check) and, for a request that may open a session, the link the
/// handler hands the new session back through.
pub(crate) struct ReqScope {
    pub(crate) caller: Option<String>,
    pub(crate) link: Option<SessionLink>,
}

r2e_core::rt::task_local! {
    /// Opened by [`TrackSessionId`] around the transport service call: rmcp
    /// handles a request — restore, handshake persistence, deletion
    /// included — inside that future, so the adapter reads it from any
    /// frame below.
    pub(crate) static REQ_SCOPE: ReqScope;
}

/// Innermost middleware on the MCP endpoint: when a request without an
/// `Mcp-Session-Id` turns out to open a session, attach the id rmcp minted
/// (response header) to the session the handler linked — so
/// [`McpSessions::get`](super::McpSessions::get) and [`McpSession::id`](super::McpSession::id) work from the moment the
/// `initialize` response is on its way, not only from the client's next
/// request. Reuses the [`SessionLink`] the auth layer deposited, if any.
/// Every call also runs inside a [`REQ_SCOPE`] carrying the caller and that
/// link, for the session store adapter.
///
/// It is also the owner check's second half: the auth and identity layers
/// refuse a foreign caller *before* the call when the session is bound on
/// this instance, but a session rmcp is restoring from the store is bound
/// only once its handshake replays — a request that arrived meanwhile was
/// let through on an unknown id. So after the call, a request that named a
/// session now bound to someone else gets the same 404, whatever rmcp
/// answered.
///
/// A hand-written tower layer (not a `Routes` middleware) because it sits in
/// the `ServiceBuilder` stack around rmcp's transport service, between the
/// auth layer and the service itself.
#[derive(Clone)]
pub(crate) struct TrackSessionId {
    pub(crate) bindings: SessionBindings,
}

impl<S> Layer<S> for TrackSessionId {
    type Service = TrackSessionIdService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TrackSessionIdService {
            inner,
            bindings: self.bindings.clone(),
        }
    }
}

/// The service [`TrackSessionId`] wraps the MCP transport in.
#[derive(Clone)]
pub(crate) struct TrackSessionIdService<S> {
    inner: S,
    bindings: SessionBindings,
}

impl<S> Service<Request> for TrackSessionIdService<S>
where
    S: Service<Request, Response = Response>,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request) -> Self::Future {
        let caller = caller_in_extensions(req.extensions()).map(|c| c.sub.to_owned());
        let presented = req
            .headers()
            .get(SESSION_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let link = if presented.is_some() {
            None
        } else {
            Some(match req.extensions().get::<SessionLink>() {
                Some(link) => link.clone(),
                None => {
                    let link = SessionLink::default();
                    req.extensions_mut().insert(link.clone());
                    link
                }
            })
        };
        let scope = ReqScope {
            caller: caller.clone(),
            link: link.clone(),
        };
        let bindings = self.bindings.clone();
        let fut = REQ_SCOPE.scope(scope, self.inner.call(req));
        Box::pin(async move {
            let response = fut.await?;
            if let Some(id) = &presented
                && !bindings.admits(id, caller.as_deref())
            {
                tracing::warn!(
                    caller = ?caller,
                    "MCP session restored for another principal while this request was \
                     in flight; response withheld"
                );
                let mut response = session_not_found();
                response
                    .extensions_mut()
                    .insert(AuthOutcome("session_mismatch"));
                return Ok(response);
            }
            if let Some(link) = link
                && let Some(id) = response
                    .headers()
                    .get(SESSION_ID_HEADER)
                    .and_then(|v| v.to_str().ok())
                && let Some(state) = link.peek()
            {
                state.set_id(id);
            }
            Ok(response)
        })
    }
}

/// `Mcp-Session-Id` → session, for the auth layer's session ↔ principal
/// check on every method (POST, the standalone SSE `GET`, `DELETE`).
///
/// Entries are weak: a session closed by `DELETE` or expired by rmcp drops
/// its handler, and the entry is pruned on the next insert. A state marked
/// [closed](SessionState::close) (a loopback connection that ended while a
/// handle still holds it) counts as gone too.
#[derive(Clone, Default)]
pub(crate) struct SessionBindings(Arc<Mutex<HashMap<Box<str>, Weak<SessionState>>>>);

impl SessionBindings {
    pub(crate) fn insert(&self, id: &str, state: Weak<SessionState>) {
        let mut map = self.0.lock().expect("MCP session bindings poisoned");
        map.retain(|_, s| s.upgrade().is_some_and(|s| !s.is_closed()));
        map.insert(id.into(), state);
    }

    /// Whether `subject` (`None` = no caller) may use the session `id`:
    /// exact equality with the subject a live session with that id is bound
    /// to, as [`SessionState::bind`] — an anonymous session is not admitted
    /// to an authenticated caller, nor the reverse. An unknown id passes
    /// (rmcp answers 404, or the handler binds a restored session).
    pub(crate) fn admits(&self, id: &str, subject: Option<&str>) -> bool {
        let state = self
            .0
            .lock()
            .expect("MCP session bindings poisoned")
            .get(id)
            .and_then(Weak::upgrade)
            .filter(|s| !s.is_closed());
        match state.as_deref() {
            Some(state) => state.subject() == subject,
            None => true,
        }
    }
}
