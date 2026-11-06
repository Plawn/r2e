//! Session persistence behind an [`McpSessionStore`]: the record format, the
//! write policy (at `initialize`, then re-armed at half-life) and the
//! adapter rmcp's transport calls to restore and delete sessions.
//!
//! rmcp owns the moments: it calls `store` once, inline, after a successful
//! handshake (before the client sees its id), `load` when a request names a
//! session this instance does not hold, and `delete` on HTTP `DELETE`. It
//! does not know the caller — the adapter reads it from the request scope
//! [`TrackSessionId`](super::TrackSessionId) opens around every request, so
//! a restored session keeps its owner and a foreign caller gets rmcp's own
//! "session not found".

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e_core::rt;
use rmcp::model::InitializeRequestParams;
use rmcp::transport::streamable_http_server::session::{
    SessionState as TransportSessionState, SessionStore as TransportSessionStore,
    SessionStoreError,
};
use serde::{Deserialize, Serialize};

use super::id::{SessionId, SessionLink, REQ_SCOPE};
use super::state::SessionState;
use super::store::McpSessionStore;

/// Bump when the record layout changes: an older record is treated as
/// absent (the client re-initializes), never misread.
const RECORD_VERSION: u8 = 1;

/// What a session record holds: enough to replay the handshake, plus the
/// subject it is bound to.
#[derive(Serialize, Deserialize)]
struct PersistedSession {
    v: u8,
    initialize: InitializeRequestParams,
    subject: Option<String>,
}

fn encode(
    initialize: &InitializeRequestParams,
    subject: Option<&str>,
) -> Result<Vec<u8>, r2e_core::json::JsonError> {
    #[derive(Serialize)]
    struct Borrowed<'a> {
        v: u8,
        initialize: &'a InitializeRequestParams,
        subject: Option<&'a str>,
    }
    r2e_core::json::to_vec(&Borrowed {
        v: RECORD_VERSION,
        initialize,
        subject,
    })
}

fn decode(id: &str, bytes: &[u8]) -> Option<PersistedSession> {
    match r2e_core::json::from_slice::<PersistedSession>(bytes) {
        Ok(record) if record.v == RECORD_VERSION => Some(record),
        Ok(record) => {
            tracing::warn!(
                session = %id,
                version = record.v,
                "MCP session record has an unknown layout version; treated as absent"
            );
            None
        }
        Err(error) => {
            tracing::warn!(
                session = %id,
                %error,
                "MCP session record is unreadable; treated as absent"
            );
            None
        }
    }
}

/// One per endpoint: the store, the TTL and the ids deleted here.
pub(crate) struct SessionPersistence {
    store: Arc<dyn McpSessionStore>,
    ttl: Duration,
    /// Ids ended by `DELETE` on this instance: a re-arm still in flight for
    /// one must not leave its record behind (checked before and after the
    /// write), and a later request naming it is not restored here. Pruned
    /// past the TTL — the record is gone by then.
    /// A `DELETE` served by another instance has no tombstone here; that
    /// window is documented, not closed (no compare-and-set in the trait).
    tombstones: Mutex<HashMap<Box<str>, rt::Instant>>,
}

impl SessionPersistence {
    pub(crate) fn new(store: Arc<dyn McpSessionStore>, ttl: Duration) -> Self {
        SessionPersistence {
            store,
            ttl,
            tombstones: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn ttl(&self) -> Duration {
        self.ttl
    }

    fn tombstoned(&self, id: &str) -> bool {
        let now = rt::Instant::now();
        let mut tombstones = self.tombstones.lock().expect("MCP tombstones poisoned");
        tombstones.retain(|_, since| now.duration_since(*since) < self.ttl);
        tombstones.contains_key(id)
    }

    fn tombstone(&self, id: &str) {
        self.tombstones
            .lock()
            .expect("MCP tombstones poisoned")
            .insert(id.into(), rt::Instant::now());
    }

    async fn write(
        &self,
        id: &str,
        initialize: &InitializeRequestParams,
        subject: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        let record = encode(initialize, subject)?;
        self.store.store(id, &record, self.ttl).await
    }

    /// Re-arm the record of `state` in the background (an active session
    /// stays restorable). Skipped for a session ended here or one whose
    /// handshake this instance has not seen; `last_persisted` moves now so
    /// concurrent requests do not each schedule a write.
    pub(crate) fn touch(self: &Arc<Self>, id: &SessionId, state: &Arc<SessionState>) {
        state.mark_persisted();
        if state.is_closed() || self.tombstoned(id) {
            return;
        }
        let Some(initialize) = state.peer_info() else {
            return;
        };
        let persistence = Arc::clone(self);
        let state = Arc::clone(state);
        let id = id.clone();
        rt::spawn_ctl(async move {
            if persistence.tombstoned(&id) {
                return;
            }
            if let Err(error) = persistence
                .write(&id, &initialize, state.subject())
                .await
            {
                tracing::warn!(session = %id, %error, "MCP session record not re-armed");
                return;
            }
            // A `DELETE` served here while the write was in flight: the
            // record it removed is back — take it out again.
            if persistence.tombstoned(&id)
                && let Err(error) = persistence.store.delete(&id).await
            {
                tracing::warn!(session = %id, %error, "MCP session record not removed");
            }
        });
    }
}

/// The `SessionStore` rmcp's transport calls; every method runs inside the
/// request scope of the HTTP request that triggered it.
pub(crate) struct TransportStore(pub(crate) Arc<SessionPersistence>);

/// The caller of the request being served: `Err` outside a request scope.
fn scoped_caller() -> Result<Option<String>, ()> {
    REQ_SCOPE.try_with(|scope| scope.caller.clone()).map_err(|_| ())
}

#[async_trait::async_trait]
impl TransportSessionStore for TransportStore {
    async fn load(&self, id: &str) -> Result<Option<TransportSessionState>, SessionStoreError> {
        let persistence = &self.0;
        if persistence.tombstoned(id) {
            return Ok(None);
        }
        // No scope ⇒ no caller to check the owner against ⇒ nothing to
        // restore (fail closed). Never the case on the HTTP path.
        let Ok(caller) = scoped_caller() else {
            tracing::debug!(session = %id, "MCP session restore outside a request scope; skipped");
            return Ok(None);
        };
        let Some(bytes) = persistence.store.load(id).await? else {
            return Ok(None);
        };
        let Some(record) = decode(id, &bytes) else {
            return Ok(None);
        };
        // Exact match, as `SessionState::bind`: an anonymous record is not
        // restorable by an authenticated caller, nor the reverse.
        if record.subject != caller {
            tracing::warn!(
                session = %id,
                caller = ?caller,
                "MCP session record presented by a different principal; not restored"
            );
            return Ok(None);
        }
        Ok(Some(TransportSessionState::new(record.initialize)))
    }

    async fn store(&self, id: &str, state: &TransportSessionState) -> Result<(), SessionStoreError> {
        // The handler linked the session it opened to this request (see
        // `R2eMcpHandler::initialize`); without it the worker is already
        // gone (a `DELETE` raced the handshake) and there is nothing to keep.
        let linked = REQ_SCOPE
            .try_with(|scope| scope.link.as_ref().and_then(SessionLink::peek))
            .ok()
            .flatten();
        let Some(session) = linked else {
            tracing::debug!(session = %id, "MCP session not linked to its request; not persisted");
            return Ok(());
        };
        // A handshake rmcp refused (`initialize` answered an error) opened
        // a session the client cannot use: nothing to restore.
        if session.is_closed() || !session.is_initialized() {
            return Ok(());
        }
        session.mark_persisted();
        self.0
            .write(id, &state.initialize_params, session.subject())
            .await
    }

    async fn delete(&self, id: &str) -> Result<(), SessionStoreError> {
        let persistence = &self.0;
        let Ok(caller) = scoped_caller() else {
            return Ok(());
        };
        // Only the owner removes the record (rmcp answers 202 either way —
        // no oracle). An unreadable record has no owner to check against:
        // it stays until its TTL, rather than going at anyone's request.
        if let Some(bytes) = persistence.store.load(id).await? {
            match decode(id, &bytes) {
                Some(record) if record.subject == caller => {}
                Some(_) => {
                    tracing::warn!(
                        session = %id,
                        caller = ?caller,
                        "MCP session record deleted by a different principal; refused"
                    );
                    return Ok(());
                }
                None => return Ok(()),
            }
        }
        persistence.tombstone(id);
        persistence.store.delete(id).await
    }
}
