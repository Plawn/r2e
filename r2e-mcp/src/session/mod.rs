//! Per-session member lists: [`McpSession`] (reshape the list from inside a
//! member), [`McpSessionInit`] (shape it when a session opens) and
//! [`McpSessions`] (reach live sessions from anywhere else).
//!
//! A session serves the catalog members of its enabled groups plus its
//! session-private members. Hidden members answer exactly like unknown
//! ones — invisible means not callable — and the auth visibility filter
//! still applies on top.
//!
//! Changes are copy-on-write: a mutation clones the session's view, applies
//! the change, rebuilds the precomputed lists and swaps them in. In-flight
//! calls keep the route they resolved; the next request sees the new list.
//! Every change that alters a family's list sends that family's
//! `notifications/*/list_changed` to the session.

mod error;
mod handle;
mod id;
mod init;
mod sessions;
mod state;

pub use error::{McpMemberError, McpSessionError};
pub use handle::McpSession;
pub use id::SessionId;
pub(crate) use id::{SessionBindings, SessionLink, TrackSessionId, SESSION_ID_HEADER};
pub(crate) use init::{erase_init, SessionInitFn};
pub use init::{McpSessionInit, SessionInit, SessionToolset};
pub use sessions::McpSessions;
pub(crate) use state::{remove_by, Draft, SessionState};
