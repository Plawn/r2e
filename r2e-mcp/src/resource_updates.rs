//! Resource-update notifications shared by application beans and MCP sessions.

use r2e_core::rt::sync::broadcast;

/// Injectable publisher for MCP `notifications/resources/updated` events.
///
/// The [`McpServer`](crate::McpServer) plugin provides one shared instance.
/// Inject it into a bean, mutate the backing resource, then call
/// [`notify`](Self::notify). Only clients subscribed to that exact URI receive
/// the notification.
#[derive(Clone)]
pub struct McpResourceUpdates {
    tx: broadcast::Sender<String>,
}

impl Default for McpResourceUpdates {
    fn default() -> Self {
        Self::new(128)
    }
}

impl McpResourceUpdates {
    /// Create a publisher retaining at most `capacity` pending updates per
    /// subscriber. Lagging sessions skip old notifications and continue with
    /// the newest update.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(1));
        Self { tx }
    }

    /// Publish an update for `uri`.
    ///
    /// Returns the number of live session listeners. No listeners is a normal
    /// condition and returns zero.
    pub fn notify(&self, uri: impl Into<String>) -> usize {
        self.tx.send(uri.into()).unwrap_or(0)
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }
}
