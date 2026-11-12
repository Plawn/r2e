//! The MCP test client: `app.mcp()` on a [`TestApp`] opens a real session on
//! the mounted endpoint through the in-process (loopback) transport and
//! wraps it in typed helpers.

use std::fmt;
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e_core::rt;
use r2e_core::rt::sync::Notify;
use r2e_test::TestApp;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, GetPromptRequestParams, GetPromptResult, JsonObject,
    NumberOrString, ProgressNotificationParam, ProgressToken, Prompt, ReadResourceRequestParams,
    RequestMetaObject, Resource, ResourceContents, ResourceTemplate,
    ResourceUpdatedNotificationParam, Tool,
};
use rmcp::service::{NotificationContext, ServiceError};
use rmcp::{ClientHandler, RoleClient};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::auth::McpPrincipal;
use crate::loopback::{LoopbackError, LoopbackIdentity, LoopbackSession, McpLoopback};

/// How long [`McpTestClient::wait_for`] waits before failing the test.
const DEFAULT_WAIT: Duration = Duration::from_secs(5);

/// Session ids for clients that did not pick one.
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

/// `app.mcp()` — the MCP counterpart of `app.get("/…")`.
///
/// ```rust,ignore
/// use r2e_mcp::testing::McpTestExt;
///
/// let mcp = app.mcp().as_user("alice", &["admin"]).await;
/// let sum: i64 = mcp.call_tool("add", json!({ "a": 2, "b": 3 })).await?;
/// ```
pub trait McpTestExt {
    /// Start an MCP session on the app's mounted endpoint. Anonymous unless
    /// an identity is chosen on the returned builder; `.await` it to connect.
    fn mcp(&self) -> McpConnect<'_>;
}

impl McpTestExt for TestApp {
    fn mcp(&self) -> McpConnect<'_> {
        McpConnect {
            app: self,
            who: Who::Anonymous,
            scopes: Vec::new(),
            session_id: None,
            progress: false,
        }
    }
}

enum Who {
    Anonymous,
    User { sub: String, roles: Vec<String> },
    Bearer(String),
    Principal(McpPrincipal),
}

/// A pending [`McpTestClient`] connection, from [`McpTestExt::mcp`].
///
/// `.await` connects and panics on refusal (the common test path);
/// [`try_connect`](Self::try_connect) returns the [`LoopbackError`] instead.
#[must_use = "an McpConnect does nothing until awaited"]
pub struct McpConnect<'a> {
    app: &'a TestApp,
    who: Who,
    scopes: Vec<String>,
    session_id: Option<String>,
    progress: bool,
}

impl McpConnect<'_> {
    /// Connect as `sub` with `roles`: a bearer token minted by the app's
    /// [`TestJwt`](r2e_test::TestJwt), exactly what
    /// [`TestRequest::as_user`](r2e_test::TestRequest::as_user) sends over
    /// HTTP. With `mcp.auth` on it goes through the endpoint's token
    /// validator (pin it with [`pin_mcp_validator`](super::pin_mcp_validator));
    /// with `mcp.auth` off the session is anonymous and only carries the
    /// `Authorization` header, as an HTTP request would.
    ///
    /// # Panics
    ///
    /// Connecting panics if the app has no `TestJwt`
    /// (`TestApp::boot` wires one; otherwise `.with_jwt(jwt)`).
    pub fn as_user(mut self, sub: &str, roles: &[&str]) -> Self {
        self.who = Who::User {
            sub: sub.to_owned(),
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
        };
        self
    }

    /// Add OAuth scopes to the token [`as_user`](Self::as_user) mints
    /// (`#[tool(scopes = "…")]`, `mcp.auth.required-scopes`).
    pub fn with_scopes(mut self, scopes: &[&str]) -> Self {
        self.scopes = scopes.iter().map(|s| (*s).to_owned()).collect();
        self
    }

    /// Connect with a raw bearer token (expired, wrong audience, minted by
    /// another issuer, …).
    pub fn bearer(mut self, token: impl Into<String>) -> Self {
        self.who = Who::Bearer(token.into());
        self
    }

    /// Connect as an already-resolved principal, bypassing token validation.
    pub fn as_principal(mut self, principal: McpPrincipal) -> Self {
        self.who = Who::Principal(principal);
        self
    }

    /// Name the session (`ToolCall::session_id`, `McpSessions::get`).
    /// Defaults to a unique `test-session-N`.
    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// Send a fresh `progressToken` with every `tools/call`,
    /// `resources/read` and `prompts/get`, as a progress-displaying client
    /// does; the server's reports are captured as
    /// [`McpNotification::Progress`].
    pub fn with_progress(mut self) -> Self {
        self.progress = true;
        self
    }

    /// Connect, returning the refusal instead of panicking — for tests
    /// asserting on [`LoopbackError::Unauthorized`] and friends.
    pub fn try_connect(
        self,
    ) -> impl Future<Output = Result<McpTestClient, LoopbackError>> + Send + 'static {
        // Everything borrowed from the app is resolved here, synchronously.
        let loopback = self.app.bean::<McpLoopback>();
        let identity = match self.who {
            Who::Anonymous => LoopbackIdentity::Anonymous,
            Who::User { sub, roles } => {
                let roles: Vec<&str> = roles.iter().map(String::as_str).collect();
                let scopes: Vec<&str> = self.scopes.iter().map(String::as_str).collect();
                let mut token = self.app.test_jwt().token_builder(&sub).roles(&roles);
                if !scopes.is_empty() {
                    token = token.scopes(&scopes);
                }
                LoopbackIdentity::Bearer(token.build())
            }
            Who::Bearer(token) => LoopbackIdentity::Bearer(token),
            Who::Principal(principal) => LoopbackIdentity::Principal(principal),
        };
        let session_id = self.session_id.unwrap_or_else(|| {
            format!(
                "test-session-{}",
                NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
            )
        });
        let progress = self.progress;
        async move {
            let log = Arc::new(NotificationLog::default());
            let recorder = NotificationRecorder {
                log: Arc::clone(&log),
            };
            let session = loopback
                .connect_with(session_id, identity, recorder)
                .await?;
            Ok(McpTestClient {
                session,
                log,
                progress: progress.then(|| AtomicU64::new(1)),
            })
        }
    }
}

impl<'a> IntoFuture for McpConnect<'a> {
    type Output = McpTestClient;
    type IntoFuture = Pin<Box<dyn Future<Output = McpTestClient> + Send + 'static>>;

    fn into_future(self) -> Self::IntoFuture {
        let connect = self.try_connect();
        Box::pin(async move {
            connect
                .await
                .unwrap_or_else(|err| panic!("MCP test client could not connect: {err}"))
        })
    }
}

/// A server notification captured by an [`McpTestClient`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum McpNotification {
    /// `notifications/tools/list_changed`.
    ToolListChanged,
    /// `notifications/resources/list_changed`.
    ResourceListChanged,
    /// `notifications/prompts/list_changed`.
    PromptListChanged,
    /// `notifications/resources/updated`.
    ResourceUpdated {
        /// The updated resource.
        uri: String,
    },
    /// `notifications/progress` (connect with
    /// [`with_progress`](McpConnect::with_progress) to request them).
    Progress {
        /// The reported progress.
        progress: f64,
        /// The reported total, if any.
        total: Option<f64>,
        /// The reported message, if any.
        message: Option<String>,
    },
}

#[derive(Default)]
struct NotificationLog {
    seen: Mutex<Vec<McpNotification>>,
    changed: Notify,
}

impl NotificationLog {
    fn push(&self, notification: McpNotification) {
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(notification);
        self.changed.notify_waiters();
    }

    fn snapshot(&self) -> Vec<McpNotification> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The rmcp client handler behind an [`McpTestClient`]: records the
/// server's notifications. Opaque — read them through the client.
pub struct NotificationRecorder {
    log: Arc<NotificationLog>,
}

impl ClientHandler for NotificationRecorder {
    fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.log.push(McpNotification::Progress {
            progress: params.progress,
            total: params.total,
            message: params.message,
        });
        std::future::ready(())
    }

    fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.log
            .push(McpNotification::ResourceUpdated { uri: params.uri });
        std::future::ready(())
    }

    fn on_resource_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.log.push(McpNotification::ResourceListChanged);
        std::future::ready(())
    }

    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.log.push(McpNotification::ToolListChanged);
        std::future::ready(())
    }

    fn on_prompt_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.log.push(McpNotification::PromptListChanged);
        std::future::ready(())
    }
}

/// What an [`McpTestClient`] call can fail with.
#[derive(Debug)]
#[non_exhaustive]
pub enum McpTestError {
    /// A JSON-RPC error response (unknown tool, invalid params, a guard's
    /// refusal, …).
    Rpc {
        /// The JSON-RPC error code (`-32602`, …).
        code: i32,
        /// The error message.
        message: String,
        /// The error's `data`, if any.
        data: Option<Value>,
    },
    /// The tool ran and answered `isError: true`.
    Tool(CallToolResult),
    /// The result could not be decoded into the requested type.
    Decode {
        /// Why decoding failed.
        message: String,
        /// The result as received.
        result: Box<CallToolResult>,
    },
    /// The session ended or the exchange broke below JSON-RPC.
    Transport(String),
}

impl McpTestError {
    /// The JSON-RPC error code, for [`Rpc`](Self::Rpc) errors.
    pub fn code(&self) -> Option<i32> {
        match self {
            Self::Rpc { code, .. } => Some(*code),
            _ => None,
        }
    }

    /// The text of an `isError: true` tool result, for [`Tool`](Self::Tool)
    /// errors.
    pub fn tool_error_text(&self) -> Option<String> {
        match self {
            Self::Tool(result) => Some(result_text(result)),
            _ => None,
        }
    }
}

impl fmt::Display for McpTestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rpc { code, message, .. } => write!(f, "JSON-RPC error {code}: {message}"),
            Self::Tool(result) => write!(f, "tool error: {}", result_text(result)),
            Self::Decode { message, .. } => write!(f, "undecodable result: {message}"),
            Self::Transport(message) => write!(f, "MCP transport error: {message}"),
        }
    }
}

impl std::error::Error for McpTestError {}

impl From<ServiceError> for McpTestError {
    fn from(err: ServiceError) -> Self {
        match err {
            ServiceError::McpError(data) => Self::Rpc {
                code: data.code.0,
                message: data.message.into_owned(),
                data: data.data,
            },
            other => Self::Transport(other.to_string()),
        }
    }
}

/// A live MCP session on a [`TestApp`]'s endpoint, with typed helpers.
///
/// The session is a real one — same catalog, guards, scopes, session hooks
/// and `McpSessions` entry as a Streamable HTTP client's — minus the wire.
/// Dropping the client ends it; [`close`](Self::close) waits for the server
/// side to finish.
pub struct McpTestClient {
    session: LoopbackSession<NotificationRecorder>,
    log: Arc<NotificationLog>,
    /// `Some` when every request carries a progress token: the next one.
    progress: Option<AtomicU64>,
}

impl McpTestClient {
    /// The session id (`ToolCall::session_id`, `McpSessions::get`).
    pub fn session_id(&self) -> &str {
        self.session.id().as_ref()
    }

    /// The underlying loopback session — the raw rmcp client, for requests
    /// the helpers do not cover.
    pub fn session(&self) -> &LoopbackSession<NotificationRecorder> {
        &self.session
    }

    /// Close the session and wait for the server side to finish; its id is
    /// free again when this returns.
    pub async fn close(self) {
        self.session.close().await;
    }

    fn meta(&self) -> Option<RequestMetaObject> {
        let next = self.progress.as_ref()?.fetch_add(1, Ordering::Relaxed);
        Some(RequestMetaObject::with_progress_token(ProgressToken(
            NumberOrString::Number(next as i64),
        )))
    }

    // ── Tools ───────────────────────────────────────────────────────────

    /// `tools/list`, every page.
    pub async fn list_tools(&self) -> Result<Vec<Tool>, McpTestError> {
        Ok(self.session.list_all_tools().await?)
    }

    /// The listed tools' names, in listing order.
    ///
    /// # Panics
    ///
    /// Panics if `tools/list` fails.
    pub async fn tool_names(&self) -> Vec<String> {
        self.list_tools()
            .await
            .unwrap_or_else(|err| panic!("tools/list failed: {err}"))
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect()
    }

    /// `tools/call`, the result untouched — `isError: true` results
    /// included. `arguments` is anything serializing to a JSON object
    /// (`json!({..})`, a `#[derive(Serialize)]` struct) or `()` for none.
    ///
    /// # Panics
    ///
    /// Panics if `arguments` does not serialize to an object.
    pub async fn call_tool_raw(
        &self,
        name: &str,
        arguments: impl Serialize,
    ) -> Result<CallToolResult, McpTestError> {
        let mut params = CallToolRequestParams::new(name.to_owned());
        params.arguments = arguments_object(arguments);
        params.meta = self.meta();
        Ok(self.session.call_tool(params).await?)
    }

    /// `tools/call`, decoded: `structuredContent` when the tool returned
    /// one, else its first text block (parsed as JSON, or taken as a string
    /// when it is not JSON). An `isError: true` result is
    /// [`McpTestError::Tool`].
    pub async fn call_tool<Out: DeserializeOwned>(
        &self,
        name: &str,
        arguments: impl Serialize,
    ) -> Result<Out, McpTestError> {
        let result = self.call_tool_raw(name, arguments).await?;
        if result.is_error == Some(true) {
            return Err(McpTestError::Tool(result));
        }
        decode(result)
    }

    /// `tools/call`, the result's text blocks joined. An `isError: true`
    /// result is [`McpTestError::Tool`].
    pub async fn call_tool_text(
        &self,
        name: &str,
        arguments: impl Serialize,
    ) -> Result<String, McpTestError> {
        let result = self.call_tool_raw(name, arguments).await?;
        if result.is_error == Some(true) {
            return Err(McpTestError::Tool(result));
        }
        Ok(result_text(&result))
    }

    // ── Resources ───────────────────────────────────────────────────────

    /// `resources/list`, every page.
    pub async fn list_resources(&self) -> Result<Vec<Resource>, McpTestError> {
        Ok(self.session.list_all_resources().await?)
    }

    /// `resources/templates/list`, every page.
    pub async fn list_resource_templates(&self) -> Result<Vec<ResourceTemplate>, McpTestError> {
        Ok(self.session.list_all_resource_templates().await?)
    }

    /// `resources/read`: the resource's contents.
    pub async fn read_resource(&self, uri: &str) -> Result<Vec<ResourceContents>, McpTestError> {
        let mut params = ReadResourceRequestParams::new(uri);
        params.meta = self.meta();
        Ok(self.session.read_resource(params).await?.contents)
    }

    /// `resources/read`, the text contents joined (blobs skipped).
    pub async fn read_resource_text(&self, uri: &str) -> Result<String, McpTestError> {
        let contents = self.read_resource(uri).await?;
        Ok(contents
            .into_iter()
            .filter_map(|content| match content {
                ResourceContents::TextResourceContents { text, .. } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    // ── Prompts ─────────────────────────────────────────────────────────

    /// `prompts/list`, every page.
    pub async fn list_prompts(&self) -> Result<Vec<Prompt>, McpTestError> {
        Ok(self.session.list_all_prompts().await?)
    }

    /// `prompts/get`. `arguments` as for
    /// [`call_tool_raw`](Self::call_tool_raw).
    ///
    /// # Panics
    ///
    /// Panics if `arguments` does not serialize to an object.
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: impl Serialize,
    ) -> Result<GetPromptResult, McpTestError> {
        let mut params = GetPromptRequestParams::new(name);
        params.arguments = arguments_object(arguments);
        params.meta = self.meta();
        Ok(self.session.get_prompt(params).await?)
    }

    // ── Notifications ───────────────────────────────────────────────────

    /// Every server notification received so far, in arrival order.
    ///
    /// Notifications are delivered asynchronously: one the server has
    /// already sent may not be here yet — use [`wait_for`](Self::wait_for)
    /// to assert that one arrives.
    pub fn notifications(&self) -> Vec<McpNotification> {
        self.log.snapshot()
    }

    /// Forget the notifications received so far.
    pub fn clear_notifications(&self) {
        self.log
            .seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// The first received notification matching `predicate`, waiting up to
    /// five seconds for it.
    ///
    /// # Panics
    ///
    /// Panics if none arrives in time.
    pub async fn wait_for(
        &self,
        predicate: impl Fn(&McpNotification) -> bool,
    ) -> McpNotification {
        match self.wait_for_within(DEFAULT_WAIT, predicate).await {
            Some(notification) => notification,
            None => panic!(
                "no matching MCP notification within {DEFAULT_WAIT:?}; received: {:?}",
                self.notifications()
            ),
        }
    }

    /// [`wait_for`](Self::wait_for) with an explicit timeout, `None` when
    /// it elapses — also the way to assert that a notification does *not*
    /// arrive.
    pub async fn wait_for_within(
        &self,
        timeout: Duration,
        predicate: impl Fn(&McpNotification) -> bool,
    ) -> Option<McpNotification> {
        let find = || self.log.snapshot().into_iter().find(|n| predicate(n));
        rt::timeout(timeout, async {
            loop {
                // Registered before the check: a push in between wakes it.
                let changed = self.log.changed.notified();
                rt::pin!(changed);
                changed.as_mut().enable();
                if let Some(found) = find() {
                    return found;
                }
                changed.await;
            }
        })
        .await
        .ok()
    }
}

impl fmt::Debug for McpTestClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpTestClient")
            .field("session_id", &self.session_id())
            .finish_non_exhaustive()
    }
}

/// `arguments` as the JSON object `tools/call` / `prompts/get` take;
/// `()` / `None` (JSON `null`) is "no arguments".
fn arguments_object(arguments: impl Serialize) -> Option<JsonObject> {
    match serde_json::to_value(arguments) {
        Ok(Value::Object(object)) => Some(object),
        Ok(Value::Null) => None,
        Ok(other) => panic!("MCP arguments must serialize to a JSON object, got: {other}"),
        Err(err) => panic!("MCP arguments failed to serialize: {err}"),
    }
}

/// The text blocks of a tool result, joined.
fn result_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|text| text.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn decode<Out: DeserializeOwned>(result: CallToolResult) -> Result<Out, McpTestError> {
    if let Some(structured) = &result.structured_content {
        return serde_json::from_value(structured.clone()).map_err(|err| McpTestError::Decode {
            message: format!("structuredContent: {err}"),
            result: Box::new(result),
        });
    }
    let Some(text) = result
        .content
        .iter()
        .find_map(|block| block.as_text().map(|text| text.text.clone()))
    else {
        return Err(McpTestError::Decode {
            message: "the result has neither structuredContent nor a text block".to_owned(),
            result: Box::new(result),
        });
    };
    // A text block is JSON for structured tools, plain text otherwise.
    match r2e_core::json::from_str::<Out>(&text) {
        Ok(out) => Ok(out),
        Err(json_err) => {
            serde_json::from_value(Value::String(text)).map_err(|_| McpTestError::Decode {
                message: format!("text content: {json_err}"),
                result: Box::new(result),
            })
        }
    }
}
