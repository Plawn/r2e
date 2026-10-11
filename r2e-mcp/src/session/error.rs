use std::fmt;

use crate::error::McpError;

/// Why a member change — on a session ([`McpSession`](super::McpSession)) or on the global
/// registry ([`McpRegistry`](crate::McpRegistry)) — was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpMemberError {
    /// The request has no MCP session to change: a sessionless client
    /// (protocol 2026-07-28+) or `mcp.stateless`. The change would be lost
    /// with the request.
    NoSession,
    /// No catalog member declares this group.
    UnknownGroup(String),
    /// A member with this key is already served by the catalog (in any
    /// group, hidden or not) or by the same holder (this session, or the
    /// global registry).
    Duplicate {
        /// `"tool"`, `"resource"`, `"resource template"` or `"prompt"`.
        kind: &'static str,
        /// The clashing name / URI.
        key: String,
    },
    /// A session-private resource URI template does not parse.
    InvalidTemplate {
        /// The rejected template.
        uri: String,
        /// Parser diagnostic.
        reason: String,
    },
    /// A dynamic member declares OAuth scopes but `mcp.auth` is disabled —
    /// the boot-time check, at runtime.
    ScopesWithoutAuth {
        /// `"tool"`, `"resource"` or `"prompt"`.
        kind: &'static str,
        /// The member name / URI.
        key: String,
    },
    /// A dynamic member's completion provider names no argument (prompt) or
    /// template variable (resource) of the member, or names one twice.
    InvalidCompletion(String),
}

/// The former name of [`McpMemberError`], kept for source compatibility.
pub type McpSessionError = McpMemberError;

impl fmt::Display for McpMemberError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            McpMemberError::NoSession => f.write_str(
                "this client has no MCP session (sessionless protocol or stateless \
                 server): the session's member list cannot be changed",
            ),
            McpMemberError::UnknownGroup(group) => write!(f, "unknown MCP group `{group}`"),
            McpMemberError::Duplicate { kind, key } => {
                write!(f, "an MCP {kind} `{key}` already exists")
            }
            McpMemberError::InvalidTemplate { uri, reason } => {
                write!(f, "invalid MCP resource URI template `{uri}`: {reason}")
            }
            McpMemberError::ScopesWithoutAuth { kind, key } => write!(
                f,
                "MCP {kind} `{key}` declares OAuth scopes but `mcp.auth` is disabled"
            ),
            McpMemberError::InvalidCompletion(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for McpMemberError {}

/// Agent-actionable refusals (no session, name clash) become domain tool
/// failures the agent can read; programming errors are internal.
impl From<McpMemberError> for McpError {
    fn from(err: McpMemberError) -> Self {
        match err {
            McpMemberError::NoSession | McpMemberError::Duplicate { .. } => {
                McpError::tool(err.to_string())
            }
            other => McpError::Internal(other.to_string()),
        }
    }
}
