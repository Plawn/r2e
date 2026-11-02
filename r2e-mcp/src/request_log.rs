//! One DEBUG line per MCP HTTP request: JSON-RPC method, HTTP status, auth
//! outcome.
//!
//! `HttpTrace` (r2e-core) already emits the generic per-request span and
//! `request completed` summary (method, route, status, latency) — its span
//! fields are a fixed, app-owned shape, so the MCP-specific part cannot be
//! added to it. This line carries only what HttpTrace cannot see, and is
//! emitted *inside* the HttpTrace span when that plugin is installed (the
//! MCP service runs within the request future), so it inherits `request_id`.
//!
//! [`McpRequestLog`] sits OUTSIDE the auth layer (it must see rejections and
//! their status). It never reads a body itself: a `POST` body is wrapped in
//! a [`BodyTap`] that copies the bytes rmcp reads (up to
//! `mcp.max-request-body-bytes`) as it reads them, and the JSON-RPC
//! `method` is parsed from that copy once the response head is back. A
//! body nobody read — the auth layer or rmcp's own header checks (`Accept`,
//! `Content-Type`, `Host`, `Origin`) rejected the request first — costs
//! nothing and logs `rpc_method="-"`, as does one over the size limit. The
//! auth outcome comes from the [`AuthOutcome`] response extension the auth
//! layer sets.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use http_body::Frame;
use r2e_core::http::response::IntoResponse;
use r2e_core::http::{Body, Bytes, Method, Request, Response};
use tower::{Layer, Service};

/// What the auth layer decided, as a response extension: `authenticated`,
/// or the rejection cause (`missing_credentials`, `unknown_kid`, …).
/// Absent when no auth layer is mounted.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AuthOutcome(pub &'static str);

/// The JSON-RPC method of a message body, bounded for logging: an unknown or
/// client-invented method is never echoed verbatim (`other`), so the log
/// line cannot be forged or blown up by the request.
pub(crate) fn rpc_method(body: &[u8]) -> &'static str {
    #[derive(serde::Deserialize)]
    struct Message {
        #[serde(default)]
        method: Option<String>,
    }
    match body.iter().find(|b| !b.is_ascii_whitespace()) {
        Some(b'[') => return "batch",
        Some(b'{') => {}
        _ => return "invalid",
    }
    match r2e_core::json::from_slice::<Message>(body) {
        Ok(Message { method: None }) => "response",
        Ok(Message {
            method: Some(method),
        }) => known_method(&method),
        Err(_) => "invalid",
    }
}

/// The client→server MCP methods (rmcp's `ClientRequest` and
/// `ClientNotification` sets), as `&'static str` (no allocation per
/// request). Server→client methods (`roots/list`, `sampling/createMessage`,
/// `elicitation/create`) are never POSTed here and are not listed.
fn known_method(method: &str) -> &'static str {
    const KNOWN: &[&str] = &[
        "initialize",
        "ping",
        "server/discover",
        "tools/list",
        "tools/call",
        "resources/list",
        "resources/templates/list",
        "resources/read",
        "resources/subscribe",
        "resources/unsubscribe",
        "subscriptions/listen",
        "prompts/list",
        "prompts/get",
        "completion/complete",
        "logging/setLevel",
        "tasks/get",
        "tasks/update",
        "tasks/cancel",
        "notifications/initialized",
        "notifications/cancelled",
        "notifications/progress",
        "notifications/roots/list_changed",
        "notifications/elicitation/response",
    ];
    KNOWN
        .iter()
        .find(|known| **known == method)
        .copied()
        .unwrap_or("other")
}

// ── The body tap ────────────────────────────────────────────────────────

/// The bytes of a request body, as whoever consumes the body reads them.
#[derive(Default)]
struct Tap {
    bytes: Vec<u8>,
    /// The body exceeded the limit: `bytes` was dropped, nothing is parsed.
    overflow: bool,
    /// The consumer reached the end of the body: `bytes` is the whole body.
    complete: bool,
}

/// A request body that copies what its consumer reads into a [`Tap`] —
/// never polls ahead of the consumer, so an unread body is never buffered.
struct BodyTap {
    inner: Body,
    tap: Arc<Mutex<Tap>>,
    limit: usize,
}

impl BodyTap {
    /// Wrap `body`; the handle reads what was consumed once the request
    /// has been answered.
    fn wrap(body: Body, limit: usize) -> (Body, Arc<Mutex<Tap>>) {
        let tap = Arc::new(Mutex::new(Tap {
            complete: http_body::Body::is_end_stream(&body),
            ..Tap::default()
        }));
        let tapped = BodyTap {
            inner: body,
            tap: Arc::clone(&tap),
            limit,
        };
        (Body::new(tapped), tap)
    }
}

/// The method the tapped body carried, or `-` when it was not read to the
/// end (rejected before the body, client gone) or exceeded the limit.
fn tapped_method(tap: &Mutex<Tap>) -> &'static str {
    let tap = tap.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if tap.complete && !tap.overflow {
        rpc_method(&tap.bytes)
    } else {
        "-"
    }
}

impl http_body::Body for BodyTap {
    type Data = Bytes;
    type Error = <Body as http_body::Body>::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        // `Body` (a boxed body) is `Unpin`, so the fields can be projected
        // without `pin-project`.
        let this = &mut *self;
        let polled = Pin::new(&mut this.inner).poll_frame(cx);
        match &polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    let mut tap = this.tap.lock().unwrap_or_else(|p| p.into_inner());
                    if !tap.overflow {
                        if tap.bytes.len() + data.len() > this.limit {
                            tap.overflow = true;
                            tap.bytes = Vec::new();
                        } else {
                            tap.bytes.extend_from_slice(data);
                        }
                    }
                }
            }
            Poll::Ready(None) => {
                this.tap.lock().unwrap_or_else(|p| p.into_inner()).complete = true;
            }
            _ => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        http_body::Body::is_end_stream(&self.inner)
    }

    fn size_hint(&self) -> http_body::SizeHint {
        http_body::Body::size_hint(&self.inner)
    }
}

// ── The layer: the log line ─────────────────────────────────────────────

/// Logs one line per MCP request (see the module docs). `limit` = the
/// transport's `max_request_body_bytes`: a body above it is never copied.
#[derive(Clone, Copy)]
pub(crate) struct McpRequestLog {
    pub limit: usize,
}

impl<S> Layer<S> for McpRequestLog {
    type Service = McpRequestLogService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        McpRequestLogService {
            inner,
            limit: self.limit,
        }
    }
}

#[derive(Clone)]
pub(crate) struct McpRequestLogService<S> {
    inner: S,
    limit: usize,
}

impl<S> Service<Request> for McpRequestLogService<S>
where
    S: Service<Request>,
    S::Response: IntoResponse,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        let http_method = req.method().clone();
        // Only a POST carries a JSON-RPC message; GET (SSE) and DELETE have
        // no body worth parsing.
        let (req, tap) = if req.method() == Method::POST {
            let (parts, body) = req.into_parts();
            let (body, tap) = BodyTap::wrap(body, self.limit);
            (Request::from_parts(parts, body), Some(tap))
        } else {
            (req, None)
        };
        let fut = self.inner.call(req);
        Box::pin(async move {
            let response = fut.await?.into_response();
            let auth = response
                .extensions()
                .get::<AuthOutcome>()
                .map_or("none", |outcome| outcome.0);
            tracing::debug!(
                http_method = %http_method,
                rpc_method = tap.as_deref().map_or("-", tapped_method),
                status = response.status().as_u16(),
                auth,
                "MCP request"
            );
            Ok(response)
        })
    }
}
