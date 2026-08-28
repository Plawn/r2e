//! Conversion of tool method return values into MCP `CallToolResult`s.

use r2e_core::http::Json;
use rmcp::model::{
    CallToolResult, ContentBlock, GetPromptResult, PromptMessage, ResourceContents, Role,
};
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

// ── Resources ──────────────────────────────────────────────────────────────

/// Types a `#[resource]` method may return.
///
/// Implemented for `String`/`&str` (one text content, MIME from the
/// `#[resource(mime_type)]` attribute, default `text/plain`), `Json<T:
/// Serialize>` (one text content, MIME defaulting to `application/json`),
/// `Vec<ResourceContents>` / `ResourceContents` (passthrough — the way to
/// return binary blobs via [`ResourceContents::blob`]), and `Result<T, E:
/// Into<McpError>>`.
pub trait IntoResourceResult {
    /// `uri` is the requested URI; `mime_type` the route's declared MIME
    /// type (both applied to text-shaped returns).
    fn into_resource_result(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<Vec<ResourceContents>, McpError>;
}

impl IntoResourceResult for Vec<ResourceContents> {
    fn into_resource_result(
        self,
        _uri: &str,
        _mime_type: Option<&str>,
    ) -> Result<Vec<ResourceContents>, McpError> {
        Ok(self)
    }
}

impl IntoResourceResult for ResourceContents {
    fn into_resource_result(
        self,
        _uri: &str,
        _mime_type: Option<&str>,
    ) -> Result<Vec<ResourceContents>, McpError> {
        Ok(vec![self])
    }
}

impl IntoResourceResult for String {
    fn into_resource_result(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<Vec<ResourceContents>, McpError> {
        // `ResourceContents::text` sets `text/plain`; honour a declared MIME.
        let mut contents = ResourceContents::text(self, uri);
        if let Some(mime) = mime_type {
            contents = contents.with_mime_type(mime);
        }
        Ok(vec![contents])
    }
}

impl IntoResourceResult for &str {
    fn into_resource_result(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<Vec<ResourceContents>, McpError> {
        self.to_string().into_resource_result(uri, mime_type)
    }
}

/// JSON contents: serialized text with MIME `application/json` unless the
/// route declares another type.
impl<T: Serialize> IntoResourceResult for Json<T> {
    fn into_resource_result(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<Vec<ResourceContents>, McpError> {
        let text = r2e_core::json::to_string(&self.0)
            .map_err(|e| McpError::Internal(format!("failed to serialize resource: {e}")))?;
        let contents = ResourceContents::text(text, uri)
            .with_mime_type(mime_type.unwrap_or("application/json"));
        Ok(vec![contents])
    }
}

impl<T: IntoResourceResult, E: Into<McpError>> IntoResourceResult for Result<T, E> {
    fn into_resource_result(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<Vec<ResourceContents>, McpError> {
        match self {
            Ok(v) => v.into_resource_result(uri, mime_type),
            Err(e) => Err(e.into()),
        }
    }
}

// ── Prompts ────────────────────────────────────────────────────────────────

/// Types a `#[prompt]` method may return.
///
/// Implemented for `String`/`&str` (a single user message),
/// `PromptMessage` / `Vec<PromptMessage>` (explicit messages), raw
/// [`GetPromptResult`], and `Result<T, E: Into<McpError>>`. The prompt's
/// description (from the doc comment) is attached unless the value already
/// carries one.
pub trait IntoPromptResult {
    fn into_prompt_result(self, description: Option<&str>) -> Result<GetPromptResult, McpError>;
}

impl IntoPromptResult for GetPromptResult {
    fn into_prompt_result(self, description: Option<&str>) -> Result<GetPromptResult, McpError> {
        let mut result = self;
        if result.description.is_none() {
            result.description = description.map(str::to_string);
        }
        Ok(result)
    }
}

impl IntoPromptResult for Vec<PromptMessage> {
    fn into_prompt_result(self, description: Option<&str>) -> Result<GetPromptResult, McpError> {
        GetPromptResult::new(self).into_prompt_result(description)
    }
}

impl IntoPromptResult for PromptMessage {
    fn into_prompt_result(self, description: Option<&str>) -> Result<GetPromptResult, McpError> {
        vec![self].into_prompt_result(description)
    }
}

/// A plain string becomes a single `user` message.
impl IntoPromptResult for String {
    fn into_prompt_result(self, description: Option<&str>) -> Result<GetPromptResult, McpError> {
        PromptMessage::new_text(Role::User, self).into_prompt_result(description)
    }
}

impl IntoPromptResult for &str {
    fn into_prompt_result(self, description: Option<&str>) -> Result<GetPromptResult, McpError> {
        self.to_string().into_prompt_result(description)
    }
}

impl<T: IntoPromptResult, E: Into<McpError>> IntoPromptResult for Result<T, E> {
    fn into_prompt_result(self, description: Option<&str>) -> Result<GetPromptResult, McpError> {
        match self {
            Ok(v) => v.into_prompt_result(description),
            Err(e) => Err(e.into()),
        }
    }
}
