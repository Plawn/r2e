//! Conversion of tool method return values into MCP `CallToolResult`s.

use r2e_core::http::Json;
use std::time::Duration;

use rmcp::model::{
    CacheScope, CallToolResult, ContentBlock, GetPromptResult, MetaObject, PromptMessage,
    ReadResourceResult, ResourceContents, Role,
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
/// return binary blobs via [`ResourceContents::blob`]), [`ResourceRead<T>`]
/// (any of the above plus the result's own fields: `ttlMs`, `cacheScope`,
/// `_meta`), a raw [`ReadResourceResult`] (sent as written), and `Result<T,
/// E: Into<McpError>>`.
pub trait IntoResourceResult {
    /// `uri` is the requested URI; `mime_type` the route's declared MIME
    /// type (both applied to text-shaped returns).
    fn into_resource_result(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<ReadResourceResult, McpError>;
}

/// The whole `resources/read` result, sent as written.
impl IntoResourceResult for ReadResourceResult {
    fn into_resource_result(
        self,
        _uri: &str,
        _mime_type: Option<&str>,
    ) -> Result<ReadResourceResult, McpError> {
        Ok(self)
    }
}

impl IntoResourceResult for Vec<ResourceContents> {
    fn into_resource_result(
        self,
        _uri: &str,
        _mime_type: Option<&str>,
    ) -> Result<ReadResourceResult, McpError> {
        Ok(ReadResourceResult::new(self))
    }
}

impl IntoResourceResult for ResourceContents {
    fn into_resource_result(
        self,
        _uri: &str,
        _mime_type: Option<&str>,
    ) -> Result<ReadResourceResult, McpError> {
        Ok(ReadResourceResult::new(vec![self]))
    }
}

impl IntoResourceResult for String {
    fn into_resource_result(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<ReadResourceResult, McpError> {
        // `ResourceContents::text` sets `text/plain`; honour a declared MIME.
        let mut contents = ResourceContents::text(self, uri);
        if let Some(mime) = mime_type {
            contents = contents.with_mime_type(mime);
        }
        Ok(ReadResourceResult::new(vec![contents]))
    }
}

impl IntoResourceResult for &str {
    fn into_resource_result(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<ReadResourceResult, McpError> {
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
    ) -> Result<ReadResourceResult, McpError> {
        let text = r2e_core::json::to_string(&self.0)
            .map_err(|e| McpError::Internal(format!("failed to serialize resource: {e}")))?;
        let contents = ResourceContents::text(text, uri)
            .with_mime_type(mime_type.unwrap_or("application/json"));
        Ok(ReadResourceResult::new(vec![contents]))
    }
}

impl<T: IntoResourceResult, E: Into<McpError>> IntoResourceResult for Result<T, E> {
    fn into_resource_result(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<ReadResourceResult, McpError> {
        match self {
            Ok(v) => v.into_resource_result(uri, mime_type),
            Err(e) => Err(e.into()),
        }
    }
}

/// A resource's contents together with the cache fields of the
/// `resources/read` result: how long the client may keep it (`ttlMs`) and who
/// may cache it (`cacheScope`).
///
/// Wraps any other resource return, which keeps its own conversion (URI,
/// declared MIME type); a field left unset keeps what the wrapped value
/// produced. Values that never change go on the attribute instead —
/// `#[resource(ttl = "30s", cache_scope = "private")]` — and this type
/// overrides them per read. For the result's `_meta`, see [`WithMeta`].
///
/// ```ignore
/// #[resource(uri = "skill://{name}/SKILL.md", mime_type = "text/markdown")]
/// async fn skill(&self, call: ResourceCall) -> Result<ResourceRead<String>, McpError> {
///     let skill = self.skills.get(&call.variables["name"]).await?;
///     Ok(ResourceRead::new(skill.body)
///         .ttl(skill.max_age)
///         .cache_scope(CacheScope::Private))
/// }
/// ```
#[derive(Debug, Clone)]
#[must_use]
pub struct ResourceRead<T> {
    contents: T,
    ttl_ms: Option<u64>,
    cache_scope: Option<CacheScope>,
}

impl<T> ResourceRead<T> {
    /// Wrap `contents` — anything a resource may return.
    pub fn new(contents: T) -> Self {
        ResourceRead {
            contents,
            ttl_ms: None,
            cache_scope: None,
        }
    }

    /// How long the client may treat the result as fresh (`ttlMs`, whole
    /// milliseconds).
    pub fn ttl(mut self, ttl: Duration) -> Self {
        self.ttl_ms = Some(duration_ms(ttl));
        self
    }

    /// Who may cache the result (`cacheScope`): [`CacheScope::Private`] for
    /// contents that depend on the caller.
    pub fn cache_scope(mut self, cache_scope: CacheScope) -> Self {
        self.cache_scope = Some(cache_scope);
        self
    }
}

impl<T: IntoResourceResult> IntoResourceResult for ResourceRead<T> {
    fn into_resource_result(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<ReadResourceResult, McpError> {
        let mut result = self.contents.into_resource_result(uri, mime_type)?;
        if self.ttl_ms.is_some() {
            result.ttl_ms = self.ttl_ms;
        }
        if self.cache_scope.is_some() {
            result.cache_scope = self.cache_scope;
        }
        Ok(result)
    }
}

/// Whole milliseconds of `duration`, saturating.
pub(crate) fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The declared cache fields of a resource (`#[resource(ttl, cache_scope)]`,
/// [`DynamicResource::ttl`](crate::DynamicResource::ttl)): applied where the
/// read itself set none.
#[doc(hidden)]
pub fn resource_cache_defaults(
    result: Result<ReadResourceResult, McpError>,
    ttl_ms: Option<u64>,
    cache_scope: Option<CacheScope>,
) -> Result<ReadResourceResult, McpError> {
    let mut result = result?;
    if result.ttl_ms.is_none() {
        result.ttl_ms = ttl_ms;
    }
    if result.cache_scope.is_none() {
        result.cache_scope = cache_scope;
    }
    Ok(result)
}

// ── Result `_meta` ─────────────────────────────────────────────────────────

/// A member's return value together with the `_meta` object of its result.
///
/// Works the same on the three families: wraps anything a `#[tool]`, a
/// `#[resource]` or a `#[prompt]` may return, which keeps its own conversion,
/// and sets `_meta` on the `tools/call`, `resources/read` or `prompts/get`
/// result. Keys the wrapped value already set are kept unless `meta` names
/// them too.
///
/// ```ignore
/// #[tool]
/// async fn search(&self, Params(q): Params<Query>) -> WithMeta<Json<Hits>> {
///     let hits = self.index.search(&q).await;
///     WithMeta::new(Json(hits), meta!({ "index/revision": self.index.revision() }))
/// }
/// ```
#[derive(Debug, Clone)]
#[must_use]
pub struct WithMeta<T> {
    value: T,
    meta: MetaObject,
}

impl<T> WithMeta<T> {
    /// Wrap `value` with the result's `_meta` — usually built with
    /// [`meta!`](crate::meta).
    pub fn new(value: T, meta: impl Into<MetaObject>) -> Self {
        WithMeta {
            value,
            meta: meta.into(),
        }
    }

    fn merge_into(meta: MetaObject, slot: &mut Option<MetaObject>) {
        match slot {
            Some(existing) => existing.0.extend(meta.0),
            None => *slot = Some(meta),
        }
    }
}

impl<T: IntoToolResult> IntoToolResult for WithMeta<T> {
    fn into_tool_result(self) -> Result<CallToolResult, McpError> {
        let mut result = self.value.into_tool_result()?;
        Self::merge_into(self.meta, &mut result.meta);
        Ok(result)
    }
}

impl<T: IntoResourceResult> IntoResourceResult for WithMeta<T> {
    fn into_resource_result(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<ReadResourceResult, McpError> {
        let mut result = self.value.into_resource_result(uri, mime_type)?;
        Self::merge_into(self.meta, &mut result.meta);
        Ok(result)
    }
}

impl<T: IntoPromptResult> IntoPromptResult for WithMeta<T> {
    fn into_prompt_result(self, description: Option<&str>) -> Result<GetPromptResult, McpError> {
        let mut result = self.value.into_prompt_result(description)?;
        Self::merge_into(self.meta, &mut result.meta);
        Ok(result)
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
