//! Durable HTTP session records: [`McpSessionStore`] and the process-local
//! [`InMemoryMcpSessionStore`].
//!
//! Without a store, a Streamable HTTP session lives in the process that
//! opened it: a restart, a deployment or a second instance behind a load
//! balancer without affinity answers every `Mcp-Session-Id` it minted with
//! `404`, and the client re-initializes. With one, the plugin writes a
//! record at `initialize` and restores the session transparently on the
//! next request that reaches an instance which does not hold it.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e_core::rt;

/// Boxed error of an [`McpSessionStore`] operation.
pub type McpSessionStoreError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// The future of one store operation.
pub type McpSessionStoreFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, McpSessionStoreError>> + Send + 'a>>;

/// Durable record of an HTTP MCP session, keyed by its `Mcp-Session-Id`.
///
/// Records are opaque bytes written by the plugin (the handshake parameters
/// and the owning subject); a backend only needs a TTL'd key/value store.
/// `ttl` is re-armed on every write: the plugin writes once at `initialize`
/// and again, in the background, when a session is used past half its TTL,
/// so an active session stays restorable and an abandoned one expires.
///
/// Install one with [`McpServer::with_session_store`](crate::McpServer::with_session_store)
/// or by providing an `Arc<dyn McpSessionStore>` bean. Errors are logged;
/// a failed `load` answers the request `500`, a failed `store` keeps the
/// session local to this instance.
pub trait McpSessionStore: Send + Sync + 'static {
    /// The record stored under `id`, `None` when unknown or expired.
    fn load(&self, id: &str) -> McpSessionStoreFuture<'_, Option<Vec<u8>>>;

    /// Write (or overwrite) the record under `id`, expiring after `ttl`.
    fn store(&self, id: &str, record: &[u8], ttl: Duration) -> McpSessionStoreFuture<'_, ()>;

    /// Remove the record under `id` (a no-op when absent).
    fn delete(&self, id: &str) -> McpSessionStoreFuture<'_, ()>;
}

impl<T: McpSessionStore + ?Sized> McpSessionStore for Arc<T> {
    fn load(&self, id: &str) -> McpSessionStoreFuture<'_, Option<Vec<u8>>> {
        (**self).load(id)
    }

    fn store(&self, id: &str, record: &[u8], ttl: Duration) -> McpSessionStoreFuture<'_, ()> {
        (**self).store(id, record, ttl)
    }

    fn delete(&self, id: &str) -> McpSessionStoreFuture<'_, ()> {
        (**self).delete(id)
    }
}

/// A process-local [`McpSessionStore`] (development and tests): sessions
/// survive nothing but the instance that wrote them. Clones share the map,
/// so two apps built on one value emulate two instances behind a shared
/// store. Expiry is checked on `load` against the runtime clock (virtual
/// under a paused test runtime).
#[derive(Clone, Default)]
pub struct InMemoryMcpSessionStore {
    records: Arc<Mutex<HashMap<Box<str>, Record>>>,
}

/// One stored record and the instant it expires.
type Record = (Vec<u8>, rt::Instant);

impl InMemoryMcpSessionStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether an unexpired record exists under `id`.
    pub fn contains(&self, id: &str) -> bool {
        self.records
            .lock()
            .expect("MCP session store poisoned")
            .get(id)
            .is_some_and(|(_, expires)| rt::Instant::now() < *expires)
    }

    /// The number of unexpired records.
    pub fn len(&self) -> usize {
        let now = rt::Instant::now();
        self.records
            .lock()
            .expect("MCP session store poisoned")
            .values()
            .filter(|(_, expires)| now < *expires)
            .count()
    }

    /// Whether no unexpired record exists.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl McpSessionStore for InMemoryMcpSessionStore {
    fn load(&self, id: &str) -> McpSessionStoreFuture<'_, Option<Vec<u8>>> {
        let found = {
            let mut records = self.records.lock().expect("MCP session store poisoned");
            match records.get(id) {
                Some((bytes, expires)) if rt::Instant::now() < *expires => Some(bytes.clone()),
                Some(_) => {
                    records.remove(id);
                    None
                }
                None => None,
            }
        };
        Box::pin(async move { Ok(found) })
    }

    fn store(&self, id: &str, record: &[u8], ttl: Duration) -> McpSessionStoreFuture<'_, ()> {
        let expires = rt::Instant::now() + ttl;
        self.records
            .lock()
            .expect("MCP session store poisoned")
            .insert(id.into(), (record.to_vec(), expires));
        Box::pin(async { Ok(()) })
    }

    fn delete(&self, id: &str) -> McpSessionStoreFuture<'_, ()> {
        self.records
            .lock()
            .expect("MCP session store poisoned")
            .remove(id);
        Box::pin(async { Ok(()) })
    }
}
