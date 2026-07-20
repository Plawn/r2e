use std::borrow::Cow;

use crate::EventMetadata;

/// A header key-value pair. Keys are `Cow<'static, str>` — static for
/// built-in headers, owned only for user-defined headers (`r2e-h-*`).
pub type HeaderPair = (Cow<'static, str>, String);

pub const HEADER_EVENT_ID: &str = "r2e-event-id";
pub const HEADER_TIMESTAMP: &str = "r2e-timestamp";
pub const HEADER_CORRELATION_ID: &str = "r2e-correlation-id";
pub const HEADER_PARTITION_KEY: &str = "r2e-partition-key";
pub const HEADER_USER_PREFIX: &str = "r2e-h-";

/// Internal request-reply correlation id (a `u128` drawn from the `event_id`
/// scheme). Distinct from [`HEADER_CORRELATION_ID`], which carries the user's
/// own `metadata.correlation_id` string and must flow through untouched — the
/// two never share a header slot.
pub const HEADER_REQUEST_ID: &str = "r2e-request-id";

/// Reply topic a responder should publish its reply to (request-reply).
pub const HEADER_REPLY_TO: &str = "r2e-reply-to";
/// Present on a reply message when the responder returned an error; its value
/// is the remote-error payload (surfaced as [`crate::EventBusError::Remote`]).
pub const HEADER_REPLY_ERROR: &str = "r2e-reply-error";

/// Lazily encode [`EventMetadata`] into key-value pairs suitable for message
/// headers in any distributed backend.
///
/// Keys are `Cow<'static, str>` — static for built-in headers, owned only
/// for user-defined headers (`r2e-h-*`).
pub fn encode_metadata(metadata: &EventMetadata) -> impl Iterator<Item = HeaderPair> + '_ {
    std::iter::once((
        Cow::Borrowed(HEADER_EVENT_ID),
        metadata.event_id.to_string(),
    ))
    .chain(std::iter::once((
        Cow::Borrowed(HEADER_TIMESTAMP),
        metadata.timestamp.to_string(),
    )))
    .chain(
        metadata
            .correlation_id
            .iter()
            .map(|cid| (Cow::Borrowed(HEADER_CORRELATION_ID), cid.clone())),
    )
    .chain(
        metadata
            .partition_key
            .iter()
            .map(|key| (Cow::Borrowed(HEADER_PARTITION_KEY), key.clone())),
    )
    .chain(metadata.headers.iter().map(|(key, value)| {
        (
            Cow::Owned(format!("{HEADER_USER_PREFIX}{key}")),
            value.clone(),
        )
    }))
}

/// Decode [`EventMetadata`] from string key-value pairs (message headers).
pub fn decode_metadata(
    pairs: impl Iterator<Item = (impl AsRef<str>, impl AsRef<str>)>,
) -> EventMetadata {
    let mut metadata = EventMetadata::new();

    for (key, value) in pairs {
        let k = key.as_ref();
        let v = value.as_ref();
        match k {
            HEADER_EVENT_ID => {
                if let Ok(id) = v.parse::<u128>() {
                    metadata.event_id = id;
                }
            }
            HEADER_TIMESTAMP => {
                if let Ok(ts) = v.parse::<u64>() {
                    metadata.timestamp = ts;
                }
            }
            HEADER_CORRELATION_ID => {
                metadata.correlation_id = Some(v.to_string());
            }
            HEADER_PARTITION_KEY => {
                metadata.partition_key = Some(v.to_string());
            }
            _ if k.starts_with(HEADER_USER_PREFIX) => {
                metadata.headers.insert(
                    k.trim_start_matches(HEADER_USER_PREFIX).to_string(),
                    v.to_string(),
                );
            }
            _ => {}
        }
    }

    metadata
}

// ── Request-reply headers ──────────────────────────────────────────────

/// The request-reply control headers carried alongside a request or a reply.
///
/// On a **request**: `request_id` is the requester's pending id and `reply_to`
/// names the topic the responder must publish the reply to. On a **reply**:
/// `request_id` echoes the request's id so the requester can match it, and
/// `reply_error` is set when the responder failed.
///
/// `request_id` lives in its own [`HEADER_REQUEST_ID`] slot, entirely separate
/// from the user's [`HEADER_CORRELATION_ID`] (`metadata.correlation_id`), which
/// passes through untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyHeaders {
    /// Internal id linking a reply to its request (`u128`, shared id scheme).
    pub request_id: u128,
    /// Topic the reply should be / was published to.
    pub reply_to: Option<String>,
    /// Remote-error payload when the responder returned an error.
    pub reply_error: Option<String>,
}

/// Lazily encode request-reply control headers into string key-value pairs.
///
/// The request id uses the dedicated [`HEADER_REQUEST_ID`] slot (never the
/// user's [`HEADER_CORRELATION_ID`]). Pass `reply_to` on a request; pass
/// `reply_error` on a failed reply.
pub fn encode_reply_headers<'a>(
    request_id: u128,
    reply_to: Option<&'a str>,
    reply_error: Option<&'a str>,
) -> impl Iterator<Item = HeaderPair> + 'a {
    std::iter::once((Cow::Borrowed(HEADER_REQUEST_ID), request_id.to_string()))
        .chain(
            reply_to
                .into_iter()
                .map(|topic| (Cow::Borrowed(HEADER_REPLY_TO), topic.to_string())),
        )
        .chain(
            reply_error
                .into_iter()
                .map(|error| (Cow::Borrowed(HEADER_REPLY_ERROR), error.to_string())),
        )
}

/// Decode request-reply control headers from message headers.
///
/// Returns `None` when there is no [`HEADER_REQUEST_ID`] — i.e. the message is
/// not part of a request-reply exchange.
pub fn decode_reply_headers(
    pairs: impl Iterator<Item = (impl AsRef<str>, impl AsRef<str>)>,
) -> Option<ReplyHeaders> {
    let mut request_id: Option<u128> = None;
    let mut reply_to: Option<String> = None;
    let mut reply_error: Option<String> = None;

    for (key, value) in pairs {
        let k = key.as_ref();
        let v = value.as_ref();
        match k {
            HEADER_REQUEST_ID => request_id = v.parse::<u128>().ok(),
            HEADER_REPLY_TO => reply_to = Some(v.to_string()),
            HEADER_REPLY_ERROR => reply_error = Some(v.to_string()),
            _ => {}
        }
    }

    request_id.map(|request_id| ReplyHeaders {
        request_id,
        reply_to,
        reply_error,
    })
}
