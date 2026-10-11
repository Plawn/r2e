use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, Weak};

use r2e_core::http::middleware::Next;
use r2e_core::http::{Request, Response};

use super::state::SessionState;

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

/// Deposited by the auth layer in the extensions of a request that carries
/// no `Mcp-Session-Id`: if it turns out to be an `initialize` that opens a
/// session, the handler hands the new session back through it, and the
/// layer keys it by the id rmcp puts in the response header.
#[derive(Clone, Default)]
pub(crate) struct SessionLink(Arc<Mutex<Option<Weak<SessionState>>>>);

impl SessionLink {
    pub(crate) fn set(&self, state: &Arc<SessionState>) {
        *self.0.lock().expect("MCP session link poisoned") = Some(Arc::downgrade(state));
    }

    pub(crate) fn take(&self) -> Option<Weak<SessionState>> {
        self.0.lock().expect("MCP session link poisoned").take()
    }

    fn peek(&self) -> Option<Arc<SessionState>> {
        self.0
            .lock()
            .expect("MCP session link poisoned")
            .as_ref()
            .and_then(Weak::upgrade)
    }
}

/// The streamable-HTTP session header (rmcp's `HEADER_SESSION_ID`).
pub(crate) const SESSION_ID_HEADER: &str = "mcp-session-id";

/// Innermost middleware on the MCP endpoint: when a request without an
/// `Mcp-Session-Id` turns out to open a session, attach the id rmcp minted
/// (response header) to the session the handler linked — so
/// [`McpSessions::get`](super::McpSessions::get) and [`McpSession::id`](super::McpSession::id) work from the moment the
/// `initialize` response is on its way, not only from the client's next
/// request. Reuses the [`SessionLink`] the auth layer deposited, if any.
pub(crate) async fn track_session_id(mut req: Request, next: Next) -> Response {
    if req.headers().contains_key(SESSION_ID_HEADER) {
        return next.run(req).await;
    }
    let link = match req.extensions().get::<SessionLink>() {
        Some(link) => link.clone(),
        None => {
            let link = SessionLink::default();
            req.extensions_mut().insert(link.clone());
            link
        }
    };
    let response = next.run(req).await;
    if let Some(id) = response
        .headers()
        .get(SESSION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        && let Some(state) = link.peek()
    {
        state.set_id(id);
    }
    response
}

/// `Mcp-Session-Id` → session, for the auth layer's session ↔ principal
/// check on every method (POST, the standalone SSE `GET`, `DELETE`).
///
/// Entries are weak: a session closed by `DELETE` or expired by rmcp drops
/// its handler, and the entry is pruned on the next insert.
#[derive(Clone, Default)]
pub(crate) struct SessionBindings(Arc<Mutex<HashMap<Box<str>, Weak<SessionState>>>>);

impl SessionBindings {
    pub(crate) fn insert(&self, id: &str, state: Weak<SessionState>) {
        let mut map = self.0.lock().expect("MCP session bindings poisoned");
        map.retain(|_, s| s.strong_count() > 0);
        map.insert(id.into(), state);
    }

    /// Whether `subject` may use the session `id`. `false` only when a live
    /// session with that id is bound to another subject; an unknown id
    /// passes (rmcp answers 404, or the handler binds a restored session).
    pub(crate) fn admits(&self, id: &str, subject: &str) -> bool {
        let state = self
            .0
            .lock()
            .expect("MCP session bindings poisoned")
            .get(id)
            .and_then(Weak::upgrade);
        match state.as_deref().and_then(SessionState::subject) {
            Some(bound) => bound == subject,
            None => true,
        }
    }
}
