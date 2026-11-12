//! Elicitation (`elicitation/create`): the [`McpClient`] a member takes as a
//! parameter to ask the user for input in the middle of a call.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use r2e_core::rt::CancelToken;
use rmcp::model::{
    ClientCapabilities, ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationSchema,
};
use rmcp::service::{ElicitationMode, Peer, RoleServer, ServiceError};
use serde::de::DeserializeOwned;

use crate::error::McpError;
use crate::mrtr::Round;

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
    back: Back,
    timeout: Duration,
    cancel: CancelToken,
}

/// How a question reaches the user. Picked per call, from what the client
/// negotiated — never by the member.
pub(crate) enum Back {
    /// No way back: a sessionless request before 2026-07-28
    /// (`mcp.stateless`), or a 2026-07-28 one while `mcp.request-state-key`
    /// is not set.
    None,
    /// A legacy MCP session: `elicitation/create` rides the call's SSE
    /// stream and the client's answer POST reaches this peer.
    Live,
    /// Protocol 2026-07-28: the call ends with the questions and the client
    /// retries it with the answers (see [`crate::mrtr`]).
    Rounds {
        round: Arc<Round>,
        /// What the client advertised on this request.
        modes: HashSet<ElicitationMode>,
    },
}

/// The elicitation modes `capabilities` advertise (rmcp's rule: a bare
/// `elicitation: {}` means form).
pub(crate) fn advertised_modes(capabilities: Option<&ClientCapabilities>) -> HashSet<ElicitationMode> {
    let mut modes = HashSet::new();
    if let Some(elicitation) = capabilities.and_then(|c| c.elicitation.as_ref()) {
        if elicitation.form.is_some() || elicitation.url.is_none() {
            modes.insert(ElicitationMode::Form);
        }
        if elicitation.url.is_some() {
            modes.insert(ElicitationMode::Url);
        }
    }
    modes
}

impl ClientChannel {
    /// No back channel — what hand-built calls carry.
    pub fn disabled() -> Self {
        ClientChannel { inner: None }
    }

    pub(crate) fn new(
        peer: &Peer<RoleServer>,
        back: Back,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Self {
        ClientChannel {
            inner: Some(Arc::new(Channel {
                peer: peer.clone(),
                back,
                timeout,
                cancel: cancel.clone(),
            })),
        }
    }
}

impl fmt::Debug for ClientChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientChannel")
            .field(
                "back",
                &match self.inner.as_ref().map(|c| &c.back) {
                    Some(Back::Live) => "live",
                    Some(Back::Rounds { .. }) => "rounds",
                    Some(Back::None) | None => "none",
                },
            )
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
/// deliverable while the call is in flight, so an `McpClient` cannot be
/// moved into a spawned task.
///
/// # Two ways to the user, one API
///
/// The transport is picked per call from what the client negotiated:
///
/// - **Live** (an MCP session, protocol up to 2025-11-25):
///   `elicitation/create` rides the call's own SSE stream and the member
///   waits for the answer.
/// - **Rounds** (protocol 2026-07-28, sessionless — MRTR): the first
///   unanswered [`elicit`](Self::elicit) returns
///   [`ElicitError::InputRequired`]; propagate it with `?`. The call ends
///   with the question, the client retries the same request with the
///   answer, and the member **runs again from the top** — `elicit` now
///   returns that answer. Requires `mcp.request-state-key`; without it a
///   2026-07-28 client gets [`ElicitError::NoChannel`].
///
/// Because a member may run several times for one call, **ask first, act
/// after**: anything done before the last `elicit` is redone on every round.
/// The key identifies a question within the call: use a distinct, stable one
/// per `elicit` site (a loop asks under `format!("item-{id}")`, not under
/// one key).
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
    /// way back and the client advertised form elicitation.
    pub fn supports_elicitation(&self) -> bool {
        self.channel_for(ElicitationMode::Form).is_ok()
    }

    /// Whether [`elicit_url`](Self::elicit_url) can reach the user: the call
    /// has a way back and the client advertised URL elicitation.
    pub fn supports_url_elicitation(&self) -> bool {
        self.channel_for(ElicitationMode::Url).is_ok()
    }

    /// Ask the user to fill in a `T` (form-mode elicitation).
    ///
    /// `T`'s JSON schema is sent as the form: the spec only allows a flat
    /// object of primitive properties (string / number / integer / boolean /
    /// enum), anything else fails with [`ElicitError::InvalidSchema`] before
    /// anything is sent.
    ///
    /// `key` names this question within the call (see the type docs): it is
    /// how a retried call finds the answer the client brings back. A live
    /// session does not need it, but the member cannot know which transport
    /// it is served on.
    ///
    /// Live, waits at most `mcp.elicitation-timeout-secs` (default 300) and
    /// stops early when the call is cancelled; in rounds, the same setting
    /// bounds how long the client has to retry.
    pub async fn elicit<T>(
        &self,
        key: &str,
        message: impl Into<String>,
    ) -> Result<Elicited<T>, ElicitError>
    where
        T: DeserializeOwned + schemars::JsonSchema,
    {
        let requested_schema = ElicitationSchema::from_type::<T>().map_err(|err| {
            ElicitError::InvalidSchema(format!(
                "`{}` is not a flat object of primitive properties: {err}",
                std::any::type_name::<T>()
            ))
        })?;
        let result = self
            .ask(
                ElicitationMode::Form,
                key,
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
    /// `elicitation_id` identifies this interaction to the client, and is
    /// the question's key within the call (see [`elicit`](Self::elicit)).
    /// `Accept` only means the user agreed to open the URL — confirm the
    /// outcome server-side.
    pub async fn elicit_url(
        &self,
        message: impl Into<String>,
        url: impl Into<String>,
        elicitation_id: impl Into<String>,
    ) -> Result<Elicited<()>, ElicitError> {
        let url = url.into();
        url::Url::parse(&url)
            .map_err(|err| ElicitError::InvalidSchema(format!("invalid URL `{url}`: {err}")))?;
        let elicitation_id = elicitation_id.into();
        let result = self
            .ask(
                ElicitationMode::Url,
                &elicitation_id.clone(),
                ElicitRequestParams::UrlElicitationParams {
                    meta: None,
                    message: message.into(),
                    url,
                    elicitation_id,
                },
            )
            .await?;
        Ok(match result.action {
            ElicitationAction::Accept => Elicited::Accept(()),
            ElicitationAction::Decline => Elicited::Decline,
            _ => Elicited::Cancel,
        })
    }

    /// The channel, once it is known to carry `mode` to the user.
    fn channel_for(&self, mode: ElicitationMode) -> Result<&'a Channel, ElicitError> {
        let channel = self.channel.inner.as_deref().ok_or(ElicitError::NoChannel)?;
        let advertised = match &channel.back {
            Back::None => return Err(ElicitError::NoChannel),
            Back::Live => channel.peer.supported_elicitation_modes().contains(&mode),
            Back::Rounds { modes, .. } => modes.contains(&mode),
        };
        if !advertised {
            return Err(ElicitError::Unsupported);
        }
        Ok(channel)
    }

    async fn ask(
        &self,
        mode: ElicitationMode,
        key: &str,
        params: ElicitRequestParams,
    ) -> Result<ElicitResult, ElicitError> {
        let channel = self.channel_for(mode)?;
        match &channel.back {
            Back::Rounds { round, .. } => match round.answer(key) {
                Some(answer) => serde_json::from_value(answer.clone())
                    .map_err(|err| ElicitError::InvalidResponse(err.to_string())),
                None => {
                    round.ask(key, params);
                    Err(ElicitError::InputRequired)
                }
            },
            _ => send(channel, params).await,
        }
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
    /// No way back to the client: a sessionless request before 2026-07-28
    /// (`mcp.stateless`), a 2026-07-28 client while `mcp.request-state-key`
    /// is not set, or a hand-built call.
    NoChannel,
    /// Rounds only (protocol 2026-07-28): the question is recorded and the
    /// call must end here so the client can answer and retry. Propagate it
    /// (`?`) — whatever the member returns instead is discarded, the client
    /// gets the question either way.
    InputRequired,
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
            ElicitError::NoChannel => f.write_str(
                "no channel to the client for elicitation: no MCP session, and \
                 `mcp.request-state-key` is not set (it enables elicitation for \
                 sessionless 2026-07-28 clients)",
            ),
            ElicitError::InputRequired => {
                f.write_str("input required: the client must answer and retry the request")
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
