//! Conversion of tool method return values into MCP `CallToolResult`s.

use r2e_core::http::Json;
use rmcp::model::{CallToolResult, ContentBlock};
use serde::Serialize;
use serde_json::Value;

use crate::error::McpError;

/// Types a `#[tool]` method may return.
///
/// Implemented for `Json<T: Serialize>` (dual encoding: `structuredContent`
/// plus a JSON text content block), `String`/`&str` (plain text), `()`
/// (empty success), `serde_json::Value`, raw [`CallToolResult`], and
/// `Result<T, E: Into<McpError>>`.
pub trait IntoToolResult {
    fn into_tool_result(self) -> Result<CallToolResult, McpError>;
}

impl IntoToolResult for CallToolResult {
    fn into_tool_result(self) -> Result<CallToolResult, McpError> {
        Ok(self)
    }
}

impl IntoToolResult for () {
    fn into_tool_result(self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![]))
    }
}

impl IntoToolResult for String {
    fn into_tool_result(self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(self)]))
    }
}

impl IntoToolResult for &str {
    fn into_tool_result(self) -> Result<CallToolResult, McpError> {
        self.to_string().into_tool_result()
    }
}

/// Dual encoding: the value lands in `structuredContent` (validated against
/// the tool's `outputSchema` when one is advertised) AND as a JSON text
/// content block for clients that only read `content`.
impl<T: Serialize> IntoToolResult for Json<T> {
    fn into_tool_result(self) -> Result<CallToolResult, McpError> {
        let value = serde_json::to_value(&self.0)
            .map_err(|e| McpError::Internal(format!("failed to serialize tool result: {e}")))?;
        value.into_tool_result()
    }
}

impl IntoToolResult for Value {
    fn into_tool_result(self) -> Result<CallToolResult, McpError> {
        let text = self.to_string();
        let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
        result.structured_content = Some(self);
        Ok(result)
    }
}

impl<T: IntoToolResult, E: Into<McpError>> IntoToolResult for Result<T, E> {
    fn into_tool_result(self) -> Result<CallToolResult, McpError> {
        match self {
            Ok(v) => v.into_tool_result(),
            Err(e) => Err(e.into()),
        }
    }
}
