use std::pin::Pin;
use std::task::{Context, Poll};

/// A finite stream that yields items from a `Vec`, then completes.
///
/// Useful for testing SSE endpoints backed by infinite broadcast streams.
/// Replace the real stream with a `FiniteStream` to make `TestApp::send()`
/// return promptly.
///
/// ```ignore
/// use r2e_test::FiniteStream;
/// use r2e_core::http::response::SseEvent;
///
/// let stream = FiniteStream::new(vec![
///     Ok(SseEvent::default().data("hello")),
///     Ok(SseEvent::default().event("update").data(r#"{"n":1}"#)),
/// ]);
/// ```
pub struct FiniteStream<T> {
    items: std::vec::IntoIter<T>,
}

impl<T> FiniteStream<T> {
    pub fn new(items: Vec<T>) -> Self {
        Self {
            items: items.into_iter(),
        }
    }
}

impl<T: Unpin> futures_core::Stream for FiniteStream<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<T>> {
        Poll::Ready(self.items.next())
    }
}

/// A parsed SSE event from a `text/event-stream` response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSseEvent {
    /// The event type (from the `event:` field), if present.
    pub event: Option<String>,
    /// The data payload (from the `data:` field(s), joined by newlines).
    pub data: String,
}

/// Parse a `text/event-stream` body into a list of SSE events.
///
/// Per the SSE spec, events are separated by double newlines. Within each
/// event block, `event:` sets the type and `data:` sets the payload (multiple
/// `data:` lines are joined with `\n`).
pub(crate) fn parse_sse_body(body: &str) -> Vec<ParsedSseEvent> {
    let mut events = Vec::new();

    for block in body.split("\n\n") {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }

        let mut event_type: Option<String> = None;
        let mut data_lines: Vec<String> = Vec::new();

        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("event:") {
                event_type = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.trim().to_string());
            }
            // Ignore other fields (id:, retry:, comments)
        }

        if !data_lines.is_empty() || event_type.is_some() {
            events.push(ParsedSseEvent {
                event: event_type,
                data: data_lines.join("\n"),
            });
        }
    }

    events
}
