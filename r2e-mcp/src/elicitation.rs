//! Elicitation (`elicitation/create`): the [`McpClient`] a member takes as a
//! parameter to ask the user for input in the middle of a call.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use r2e_core::rt::CancelToken;
use rmcp::model::{ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationSchema};
use rmcp::service::{ElicitationMode, Peer, RoleServer, ServiceError};
use serde::de::DeserializeOwned;

use crate::error::McpError;

/// Default `mcp.elicitation-timeout-secs`: how long a member waits for the
/// user's answer.
pub(crate) const DEFAULT_ELICITATION_TIMEOUT: Duration = Duration::from_secs(300);

/// The per-call back channel an [`McpClient`] borrows. Carried by
/// `ToolCall`/`ResourceCall`/`PromptCall`; reach it through their
/// `client()` method or an `McpClient` member parameter.
#[doc(hidden)]
#[derive(Clone)]
pub struct ClientChannel {
    inner: Option<Arc<Channel>>,
}

struct Channel {
    peer: Peer<RoleServer>,
    /// A legacy MCP session: the client's answer POST reaches this peer.
    /// Sessionless requests (`mcp.stateless`, 2026-07-28 per-request
    /// negotiation) are served over a one-shot transport that drops it.
    live: bool,
    timeout: Duration,
    cancel: CancelToken,
}

impl ClientChannel {
    /// No back channel — what hand-built calls carry.
    pub fn disabled() -> Self {
        ClientChannel { inner: None }
    }

    pub(crate) fn new(
        peer: &Peer<RoleServer>,
        live: bool,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Self {
        ClientChannel {
            inner: Some(Arc::new(Channel {
                peer: peer.clone(),
                live,
                timeout,
                cancel: cancel.clone(),
            })),
        }
    }
}

impl fmt::Debug for ClientChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientChannel")
            .field("live", &self.inner.as_ref().is_some_and(|c| c.live))
            .finish_non_exhaustive()
    }
}

/// Requests the member can send back to the client while it handles the
/// call — today, elicitation.
///
/// Take it as a member parameter (`client: McpClient<'_>`) on a tool,
/// resource or prompt, or borrow one from the raw call
/// ([`ToolCall::client`](crate::ToolCall::client)).
///
/// The lifetime ties it to the call: a request to the client is only
/// deliverable while the call is in flight (it rides the call's own SSE
/// stream), so an `McpClient` cannot be moved into a spawned task.
///
/// Live elicitation needs an MCP session (the client's answer arrives on a
/// separate POST routed by `Mcp-Session-Id`). Under `mcp.stateless`, and for
/// sessionless 2026-07-28 clients, every request fails fast with
/// [`ElicitError::NoChannel`] instead of hanging.
#[derive(Clone, Copy)]
pub struct McpClient<'a> {
    channel: &'a ClientChannel,
}

impl<'a> McpClient<'a> {
    #[doc(hidden)]
    pub fn new(channel: &'a ClientChannel) -> Self {
        McpClient { channel }
    }

    /// Whether [`elicit`](Self::elicit) can reach the user: the call has a
    /// live back channel and the client advertised form elicitation.
    pub fn supports_elicitation(&self) -> bool {
        self.live().is_some_and(|c| {
            c.peer
                .supported_elicitation_modes()
                .contains(&ElicitationMode::Form)
        })
    }

    /// Whether [`elicit_url`](Self::elicit_url) can reach the user: the call
    /// has a live back channel and the client advertised URL elicitation.
    pub fn supports_url_elicitation(&self) -> bool {
        self.live().is_some_and(|c| {
            c.peer
                .supported_elicitation_modes()
                .contains(&ElicitationMode::Url)
        })
    }

    /// Ask the user to fill in a `T` (form-mode elicitation).
    ///
    /// `T`'s JSON schema is sent as the form: the spec only allows a flat
    /// object of primitive properties (string / number / integer / boolean /
    /// enum), anything else fails with [`ElicitError::InvalidSchema`] before
    /// anything is sent.
    ///
    /// Waits at most `mcp.elicitation-timeout-secs` (default 300) and stops
    /// early when the call is cancelled.
    pub async fn elicit<T>(&self, message: impl Into<String>) -> Result<Elicited<T>, ElicitError>
    where
        T: DeserializeOwned + schemars::JsonSchema,
    {
        let requested_schema = ElicitationSchema::from_type::<T>().map_err(|err| {
            ElicitError::InvalidSchema(format!(
                "`{}` is not a flat object of primitive properties: {err}",
                std::any::type_name::<T>()
            ))
        })?;
        let channel = self.channel_for(ElicitationMode::Form)?;
        let result = send(
            channel,
            ElicitRequestParams::FormElicitationParams {
                meta: None,
                message: message.into(),
                requested_schema,
            },
        )
        .await?;
        match result.action {
            ElicitationAction::Accept => {
                let content = result.content.ok_or_else(|| {
                    ElicitError::InvalidResponse("accepted without content".into())
                })?;
                serde_json::from_value(content)
                    .map(Elicited::Accept)
                    .map_err(|err| ElicitError::InvalidResponse(err.to_string()))
            }
            ElicitationAction::Decline => Ok(Elicited::Decline),
            // `Cancel`, and any action a newer protocol adds: no answer.
            _ => Ok(Elicited::Cancel),
        }
    }

    /// Send the user to `url` for an out-of-band interaction (URL-mode
    /// elicitation: OAuth consent, payment, …) and wait for them to
    /// acknowledge it.
    ///
    /// `elicitation_id` identifies this interaction to the client. `Accept`
    /// only means the user agreed to open the URL — confirm the outcome
    /// server-side.
    pub async fn elicit_url(
        &self,
        message: impl Into<String>,
        url: impl Into<String>,
        elicitation_id: impl Into<String>,
    ) -> Result<Elicited<()>, ElicitError> {
        let url = url.into();
        url::Url::parse(&url)
            .map_err(|err| ElicitError::InvalidSchema(format!("invalid URL `{url}`: {err}")))?;
        let channel = self.channel_for(ElicitationMode::Url)?;
        let result = send(
            channel,
            ElicitRequestParams::UrlElicitationParams {
                meta: None,
                message: message.into(),
                url,
                elicitation_id: elicitation_id.into(),
            },
        )
        .await?;
        Ok(match result.action {
            ElicitationAction::Accept => Elicited::Accept(()),
            ElicitationAction::Decline => Elicited::Decline,
            _ => Elicited::Cancel,
        })
    }

    fn live(&self) -> Option<&'a Channel> {
        self.channel.inner.as_deref().filter(|c| c.live)
    }

    fn channel_for(&self, mode: ElicitationMode) -> Result<&'a Channel, ElicitError> {
        let channel = self.live().ok_or(ElicitError::NoChannel)?;
        if !channel.peer.supported_elicitation_modes().contains(&mode) {
            return Err(ElicitError::Unsupported);
        }
        Ok(channel)
    }
}

impl fmt::Debug for McpClient<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpClient")
            .field("channel", self.channel)
            .finish()
    }
}

async fn send(channel: &Channel, params: ElicitRequestParams) -> Result<ElicitResult, ElicitError> {
    let request = channel
        .peer
        .create_elicitation_with_timeout(params, Some(channel.timeout));
    let result = r2e_core::rt::select! {
        result = request => result,
        _ = channel.cancel.cancelled() => return Err(ElicitError::Cancelled),
    };
    result.map_err(|err| match err {
        ServiceError::Timeout { .. } => ElicitError::Timeout,
        ServiceError::Cancelled { .. } => ElicitError::Cancelled,
        other => ElicitError::Transport(other.to_string()),
    })
}

/// The user's answer to an elicitation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Elicited<T> {
    /// The user submitted the form (URL mode: agreed to open the URL).
    Accept(T),
    /// The user explicitly refused.
    Decline,
    /// The user dismissed the request without choosing.
    Cancel,
}

/// Why an elicitation got no answer from the user.
///
/// `?` in a member converts it into an [`McpError`]: a tool error result the
/// agent can read (the setup problems [`InvalidSchema`](Self::InvalidSchema)
/// and [`Transport`](Self::Transport) become internal errors).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ElicitError {
    /// The client did not advertise this elicitation mode.
    Unsupported,
    /// No back channel to the client: no MCP session (`mcp.stateless`, a
    /// sessionless 2026-07-28 client) or a hand-built call.
    NoChannel,
    /// The user did not answer within `mcp.elicitation-timeout-secs`.
    Timeout,
    /// The call was cancelled while waiting.
    Cancelled,
    /// The requested type is not a valid elicitation form (or the URL is
    /// malformed) — a server-side bug.
    InvalidSchema(String),
    /// The answer does not match the requested type.
    InvalidResponse(String),
    /// The request could not be delivered.
    Transport(String),
}

impl fmt::Display for ElicitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ElicitError::Unsupported => f.write_str("the client does not support this elicitation"),
            ElicitError::NoChannel => {
                f.write_str("no channel to the client for elicitation (no MCP session)")
            }
            ElicitError::Timeout => f.write_str("the user did not answer in time"),
            ElicitError::Cancelled => f.write_str("the call was cancelled during elicitation"),
            ElicitError::InvalidSchema(m) => write!(f, "invalid elicitation request: {m}"),
            ElicitError::InvalidResponse(m) => write!(f, "invalid elicitation answer: {m}"),
            ElicitError::Transport(m) => write!(f, "elicitation not delivered: {m}"),
        }
    }
}

impl std::error::Error for ElicitError {}

impl From<ElicitError> for McpError {
    fn from(err: ElicitError) -> Self {
        match err {
            ElicitError::InvalidSchema(_) | ElicitError::Transport(_) => {
                McpError::Internal(err.to_string())
            }
            other => McpError::tool(other.to_string()),
        }
    }
}
