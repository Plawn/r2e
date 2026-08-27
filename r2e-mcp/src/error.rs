//! MCP tool errors and their mapping onto the wire.
//!
//! Two failure planes exist in MCP, and the mapping is part of the contract:
//!
//! - **Domain failures** ([`McpError::Tool`]) are *results*: they become
//!   `CallToolResult { is_error: true }` so the calling agent can read the
//!   message and self-correct (retry with different arguments, pick another
//!   tool).
//! - **Protocol failures** (every other variant) become JSON-RPC error
//!   responses (`ErrorData`) — the call itself was invalid or could not be
//!   served.

use std::borrow::Cow;
use std::fmt;

use r2e_core::HttpError;
use rmcp::model::{CallToolResult, ContentBlock, ErrorCode, ErrorData};

/// Error type returned by MCP tool methods.
///
/// Return `Err(McpError::tool(...))` (or any `Result<_, E: Into<McpError>>`)
/// from a `#[tool]` method; the dispatcher maps it per the table on
/// [the module docs](self).
#[derive(Debug, Clone)]
pub enum McpError {
    /// A domain-level tool failure, reported **inside** the tool result
    /// (`is_error: true`) so agents can read it and adapt.
    Tool {
        /// Human/agent-readable failure description.
        message: String,
        /// Optional structured detail, surfaced as `structuredContent`.
        data: Option<serde_json::Value>,
    },
    /// The tool arguments failed schema/deserialization or semantic
    /// validation (JSON-RPC `-32602`).
    InvalidParams(String),
    /// The addressed entity does not exist (JSON-RPC `-32002`,
    /// `RESOURCE_NOT_FOUND`).
    NotFound(String),
    /// Authentication is missing or invalid.
    Unauthorized(String),
    /// The caller is authenticated but not allowed.
    Forbidden(String),
    /// Unexpected server-side failure (JSON-RPC `-32603`).
    Internal(String),
}

impl McpError {
    /// A domain-level tool failure (see [`McpError::Tool`]).
    pub fn tool(message: impl Into<String>) -> Self {
        McpError::Tool {
            message: message.into(),
            data: None,
        }
    }

    /// A domain-level tool failure with structured detail.
    pub fn tool_with_data(message: impl Into<String>, data: serde_json::Value) -> Self {
        McpError::Tool {
            message: message.into(),
            data: Some(data),
        }
    }

    /// Build a [`McpError::Tool`] from any displayable error.
    pub fn tool_from(err: impl fmt::Display) -> Self {
        McpError::tool(err.to_string())
    }

    /// Map this error onto the wire: domain failures become an
    /// `is_error` tool result (`Ok`), protocol failures a JSON-RPC error
    /// (`Err`). See the module docs for why the split matters.
    pub fn into_call_result(self) -> Result<CallToolResult, ErrorData> {
        match self {
            McpError::Tool { message, data } => {
                let mut result = CallToolResult::error(vec![ContentBlock::text(message)]);
                result.structured_content = data;
                Ok(result)
            }
            McpError::InvalidParams(msg) => {
                Err(ErrorData::new(ErrorCode::INVALID_PARAMS, msg, None))
            }
            McpError::NotFound(msg) => {
                Err(ErrorData::new(ErrorCode::RESOURCE_NOT_FOUND, msg, None))
            }
            McpError::Unauthorized(msg) => Err(ErrorData::new(
                ErrorCode::INVALID_REQUEST,
                msg,
                Some(serde_json::Value::String("unauthorized".into())),
            )),
            McpError::Forbidden(msg) => Err(ErrorData::new(
                ErrorCode::INVALID_REQUEST,
                msg,
                Some(serde_json::Value::String("forbidden".into())),
            )),
            McpError::Internal(msg) => Err(ErrorData::new(ErrorCode::INTERNAL_ERROR, msg, None)),
        }
    }
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            McpError::Tool { message, .. } => write!(f, "tool error: {message}"),
            McpError::InvalidParams(m) => write!(f, "invalid params: {m}"),
            McpError::NotFound(m) => write!(f, "not found: {m}"),
            McpError::Unauthorized(m) => write!(f, "unauthorized: {m}"),
            McpError::Forbidden(m) => write!(f, "forbidden: {m}"),
            McpError::Internal(m) => write!(f, "internal error: {m}"),
        }
    }
}

impl std::error::Error for McpError {}

/// [`HttpError`] maps by status so services shared between HTTP controllers
/// and MCP tools (`Result<_, HttpError>` methods) keep their semantics: 401 →
/// [`McpError::Unauthorized`], 403 → [`Forbidden`](McpError::Forbidden),
/// 404 → [`NotFound`](McpError::NotFound), 400/422 →
/// [`InvalidParams`](McpError::InvalidParams), 5xx →
/// [`Internal`](McpError::Internal), anything else a domain
/// [`Tool`](McpError::Tool) failure.
impl From<HttpError> for McpError {
    fn from(err: HttpError) -> Self {
        fn msg(m: Cow<'static, str>) -> String {
            m.into_owned()
        }
        match err {
            HttpError::NotFound(m) => McpError::NotFound(msg(m)),
            HttpError::Unauthorized(m) => McpError::Unauthorized(msg(m)),
            HttpError::Forbidden(m) => McpError::Forbidden(msg(m)),
            HttpError::BadRequest(m) => McpError::InvalidParams(msg(m)),
            HttpError::Internal(m) => McpError::Internal(msg(m)),
            HttpError::Validation(v) => match serde_json::to_value(&v) {
                Ok(data) => McpError::Tool {
                    message: "validation failed".to_string(),
                    data: Some(data),
                },
                Err(_) => McpError::InvalidParams("validation failed".to_string()),
            },
            HttpError::Custom { status, body } => {
                let message = body
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("request failed")
                    .to_string();
                from_status(status.as_u16(), message)
            }
            HttpError::WithSource {
                status, message, ..
            } => from_status(status.as_u16(), message.into_owned()),
            // `HttpError` is #[non_exhaustive]; future variants degrade to a
            // generic internal error rather than breaking this crate.
            other => McpError::Internal(other.to_string()),
        }
    }
}

fn from_status(status: u16, message: String) -> McpError {
    match status {
        401 => McpError::Unauthorized(message),
        403 => McpError::Forbidden(message),
        404 => McpError::NotFound(message),
        400 | 422 => McpError::InvalidParams(message),
        500..=599 => McpError::Internal(message),
        _ => McpError::tool(message),
    }
}
