use std::sync::{Arc, Mutex, Weak};

use r2e_core::rt::sync::broadcast;

use super::handle::McpSession;
use super::state::SessionState;
use crate::catalog::Changed;

struct SessionsInner {
    live: Mutex<Vec<Weak<SessionState>>>,
    /// Fan-out to sessionless `subscriptions/listen` streams.
    changed: broadcast::Sender<Changed>,
}

/// Every live MCP session of the endpoint — a bean provided by the
/// [`McpServer`](crate::McpServer) plugin.
///
/// For changes decided outside a member: an admin endpoint, a
/// `#[consumer]` reacting to a permission change, a scheduled cleanup.
///
/// ```ignore
/// for session in sessions.for_subject(&user_id) {
///     session.disable_group("billing")?;
/// }
/// ```
///
/// Only real sessions (stateful serving) are listed, from their
/// `initialize` handshake on.
/// Sessions are held weakly: closing one drops it from the registry.
#[derive(Clone)]
pub struct McpSessions {
    inner: Arc<SessionsInner>,
}

impl Default for McpSessions {
    fn default() -> Self {
        let (changed, _) = broadcast::channel(16);
        McpSessions {
            inner: Arc::new(SessionsInner {
                live: Mutex::new(Vec::new()),
                changed,
            }),
        }
    }
}

impl McpSessions {
    pub(crate) fn register(&self, state: &Arc<SessionState>) {
        let mut live = self
            .inner
            .live
            .lock()
            .expect("MCP session registry poisoned");
        live.retain(|s| s.strong_count() > 0);
        live.push(Arc::downgrade(state));
    }

    pub(crate) fn subscribe_changes(&self) -> broadcast::Receiver<Changed> {
        self.inner.changed.subscribe()
    }

    /// Tell every client — live sessions and sessionless
    /// `subscriptions/listen` streams — to re-list the `changed` families.
    pub(crate) fn notify(&self, changed: Changed) {
        if !changed.any() {
            return;
        }
        for state in self.live() {
            state.notify(changed);
        }
        let _ = self.inner.changed.send(changed);
    }

    fn live(&self) -> Vec<Arc<SessionState>> {
        let mut live = self
            .inner
            .live
            .lock()
            .expect("MCP session registry poisoned");
        live.retain(|s| s.strong_count() > 0);
        live.iter().filter_map(Weak::upgrade).collect()
    }

    /// Every live session.
    pub fn all(&self) -> Vec<McpSession> {
        self.live().into_iter().map(McpSession::new).collect()
    }

    /// The live session with the transport id `id`, if any.
    pub fn get(&self, id: &str) -> Option<McpSession> {
        self.live()
            .into_iter()
            .find(|state| state.id().is_some_and(|known| known == id))
            .map(McpSession::new)
    }

    /// The live sessions bound to `subject` (the principal's `sub`).
    pub fn for_subject(&self, subject: &str) -> Vec<McpSession> {
        self.live()
            .into_iter()
            .filter(|state| state.subject() == Some(subject))
            .map(McpSession::new)
            .collect()
    }

    /// The number of live sessions.
    pub fn len(&self) -> usize {
        self.live().len()
    }

    /// Whether no session is live.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Tell every client to re-list tools, resources and prompts — after a
    /// change that alters what [`McpSessionInit`](super::McpSessionInit) or the auth filter would
    /// compute (e.g. a role granted). Also reaches sessionless clients
    /// holding a `subscriptions/listen` stream.
    pub fn notify_list_changed(&self) {
        self.notify(Changed::ALL);
    }
}
