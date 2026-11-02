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
//! Two layers cooperate:
//!
//! - [`McpRequestLog`] (outside the auth layer) deposits an [`RpcMethodSlot`]
//!   in the request extensions and, once the response head is back, logs
//!   the line — the auth outcome comes from the [`AuthOutcome`] response
//!   extension the auth layer sets.
//! - [`PeekRpcMethod`] (inside the auth layer, so only AUTHENTICATED bodies
//!   are ever buffered here) reads the JSON-RPC `method` of a `POST` whose
//!   exact length (`Content-Length`) is within `mcp.max-request-body-bytes` and fills the
//!   slot. rmcp buffers that body anyway; the bytes are handed back to it
//!   unchanged. A chunked body, an oversized one, or a rejected request logs
//!   `rpc_method="-"`.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};

use r2e_core::http::body::to_bytes;
use r2e_core::http::response::IntoResponse;
use r2e_core::http::{Body, Method, Request, Response, StatusCode};
use tower::{Layer, Service};

/// What the auth layer decided, as a response extension: `authenticated`,
/// or the rejection cause (`missing_credentials`, `unknown_kid`, …).
/// Absent when no auth layer is mounted.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AuthOutcome(pub &'static str);

/// Request extension filled by [`PeekRpcMethod`].
#[derive(Clone, Default)]
pub(crate) struct RpcMethodSlot(Arc<OnceLock<&'static str>>);

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

/// The MCP methods, as `&'static str` (no allocation per request).
fn known_method(method: &str) -> &'static str {
    const KNOWN: &[&str] = &[
        "initialize",
        "ping",
        "tools/list",
        "tools/call",
        "resources/list",
        "resources/templates/list",
        "resources/read",
        "resources/subscribe",
        "resources/unsubscribe",
        "prompts/list",
        "prompts/get",
        "completion/complete",
        "logging/setLevel",
        "notifications/initialized",
        "notifications/cancelled",
        "notifications/progress",
        "notifications/roots/list_changed",
        "roots/list",
        "sampling/createMessage",
        "elicitation/create",
    ];
    KNOWN
        .iter()
        .find(|known| **known == method)
        .copied()
        .unwrap_or("other")
}

// ── Outer layer: the log line ───────────────────────────────────────────

/// Logs one line per MCP request (see the module docs).
#[derive(Clone, Copy, Default)]
pub(crate) struct McpRequestLog;

impl<S> Layer<S> for McpRequestLog {
    type Service = McpRequestLogService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        McpRequestLogService { inner }
    }
}

#[derive(Clone)]
pub(crate) struct McpRequestLogService<S> {
    inner: S,
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

    fn call(&mut self, mut req: Request) -> Self::Future {
        let http_method = req.method().clone();
        let slot = RpcMethodSlot::default();
        req.extensions_mut().insert(slot.clone());
        let fut = self.inner.call(req);
        Box::pin(async move {
            let response = fut.await?.into_response();
            let auth = response
                .extensions()
                .get::<AuthOutcome>()
                .map_or("none", |outcome| outcome.0);
            tracing::debug!(
                http_method = %http_method,
                rpc_method = slot.0.get().copied().unwrap_or("-"),
                status = response.status().as_u16(),
                auth,
                "MCP request"
            );
            Ok(response)
        })
    }
}

// ── Inner layer: peek the JSON-RPC method ───────────────────────────────

/// Reads the JSON-RPC method into the [`RpcMethodSlot`] (see the module
/// docs). `limit` = the transport's `max_request_body_bytes`.
#[derive(Clone, Copy)]
pub(crate) struct PeekRpcMethod {
    pub limit: usize,
}

impl<S> Layer<S> for PeekRpcMethod {
    type Service = PeekRpcMethodService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        PeekRpcMethodService {
            inner,
            limit: self.limit,
        }
    }
}

#[derive(Clone)]
pub(crate) struct PeekRpcMethodService<S> {
    inner: S,
    limit: usize,
}

impl<S> Service<Request> for PeekRpcMethodService<S>
where
    S: Service<Request, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        let slot = req.extensions().get::<RpcMethodSlot>().cloned();
        // Exact only for a `Content-Length` body (or a fully buffered one);
        // a chunked stream is left untouched.
        let length = http_body::Body::size_hint(req.body())
            .exact()
            .and_then(|n| usize::try_from(n).ok());
        let (Some(slot), Some(length)) = (slot, length) else {
            return Box::pin(self.inner.call(req));
        };
        if req.method() != Method::POST || length > self.limit {
            return Box::pin(self.inner.call(req));
        }

        // Clone dance: the ready clone serves this call.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let limit = self.limit;
        Box::pin(async move {
            let (parts, body) = req.into_parts();
            // hyper enforces Content-Length, so this only fails when the
            // client goes away mid-body — rmcp could not have read it either.
            let Ok(bytes) = to_bytes(body, limit).await else {
                let mut response = Response::new("Bad Request: failed to read body".into());
                *response.status_mut() = StatusCode::BAD_REQUEST;
                return Ok(response);
            };
            let _ = slot.0.set(rpc_method(&bytes));
            inner
                .call(Request::from_parts(parts, Body::from(bytes)))
                .await
        })
    }
}
