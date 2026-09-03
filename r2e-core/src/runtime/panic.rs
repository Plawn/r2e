//! Panic capture for the HTTP stack: the layer that turns a panicking handler
//! into a JSON 500 **and** into something an operator can see.
//!
//! # Why R2E owns this instead of `tower_http::catch_panic`
//!
//! `tower_http`'s handler only ever receives the unwind payload, so a response
//! is all it can produce: no log line, no route, no hook. This layer keeps the
//! same response contract and adds the two things a service actually needs
//! when a handler panics — one structured `tracing::error!` and one optional
//! application callback ([`PanicHook`]).
//!
//! # Where it sits, and why that is the whole point
//!
//! [`HttpTraceResponseFuture`](crate::runtime::http_trace::HttpTraceResponseFuture)
//! enters the request span around the *inner* poll only, so an unwind crossing
//! it drops the span guard on the way out. A catch-panic layer installed
//! **outside** `HttpTrace` therefore runs with no request span current — its
//! log line cannot carry `request_id` — and, worse, the unwind has already
//! skipped `HttpTrace`'s response path and the metrics layer: no `request
//! completed` line, no RED series, nothing to alert on.
//!
//! So the primary install slot is **innermost**, below every layer added by
//! [`add_layer`](crate::plugin::PluginBuildContext::add_layer) (tracing and
//! metrics included). A handler panic becomes an ordinary 500 *there*, and
//! that 500 then flows back out through the instrumentation like any other
//! error response. The outermost install stays as a bare last-resort net for
//! panics raised by the outer layers themselves; it never fires for a handler
//! panic, so a panic still produces exactly one error line.

use std::any::Any;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use pin_project_lite::pin_project;
use tower::{Layer, Service};

use crate::http::extract::MatchedPath;
use crate::http::header::HttpRequest as Request;
use crate::http::response::Response;
use crate::http::StatusCode;

/// `tracing` target of the panic event.
pub const PANIC_TARGET: &str = "r2e::panic";

/// Message used when the unwind payload is neither `&str` nor `String`.
pub const UNKNOWN_PAYLOAD: &str = "<non-string panic payload>";

/// What a [`PanicHook`] is told about a caught panic.
///
/// Deliberately minimal — a message and, when routing already happened, the
/// bounded route template. Enough to drive a per-route counter without
/// freezing a wide API, and with nothing request-borne in it: never a body,
/// never a header, never a path parameter.
#[derive(Debug, Clone, Copy)]
pub struct PanicReport<'a> {
    message: &'a str,
    route: Option<&'a str>,
}

impl<'a> PanicReport<'a> {
    /// The panic message, downcast from the unwind payload.
    ///
    /// [`UNKNOWN_PAYLOAD`] when the payload was neither `&'static str` nor
    /// `String` (`panic_any` with a custom type).
    pub fn message(&self) -> &'a str {
        self.message
    }

    /// The matched route template (`/users/{id}`), when the panic happened
    /// after routing.
    ///
    /// `None` for the outermost install slot (routing has not happened from
    /// its point of view) and for a request that matched no route.
    pub fn route(&self) -> Option<&'a str> {
        self.route
    }
}

/// Application callback invoked once per caught panic, before the 500 is built.
///
/// Registered with
/// [`AppBuilder::on_panic`](crate::builder::AppBuilder::on_panic). R2E
/// deliberately does not increment a metric of its own here: every service has
/// its own registry and its own metric prefix, so counting is the app's call.
///
/// The hook runs **inside the unwind's catch**, on the request task, with the
/// request span current. Keep it short and non-blocking, and do not panic in
/// it — a panic inside the hook escapes this layer.
pub type PanicHook = Arc<dyn Fn(&PanicReport<'_>) + Send + Sync>;

/// Downcast an unwind payload to its message.
pub fn panic_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.as_str()
    } else {
        UNKNOWN_PAYLOAD
    }
}

/// The JSON 500 a caught panic answers with. Byte-identical to what the
/// previous `tower_http` handler produced — the client contract is unchanged.
fn panic_response() -> Response {
    crate::http::response::static_json(
        StatusCode::INTERNAL_SERVER_ERROR,
        r#"{"error":"Internal server error"}"#,
    )
}

/// Emit the error line, run the hook, and build the 500.
///
/// Called from inside the catch, so the request span (when this layer sits
/// below [`HttpTrace`](crate::builtins::HttpTrace)) is still current and the
/// event inherits its `request_id` / `route` fields.
fn handle_panic(
    payload: Box<dyn Any + Send>,
    route: Option<&MatchedPath>,
    hook: Option<&PanicHook>,
) -> Response {
    let message = panic_message(payload.as_ref());
    let route = route.map(MatchedPath::as_str);

    tracing::error!(
        target: PANIC_TARGET,
        panic_message = %message,
        route = route.unwrap_or(crate::http::labels::UNMATCHED_PATH_LABEL),
        "handler panicked; responding 500"
    );

    if let Some(hook) = hook {
        hook(&PanicReport { message, route });
    }

    panic_response()
}

/// [`Layer`] installing [`CatchPanic`].
#[derive(Clone, Default)]
pub struct CatchPanicLayer {
    hook: Option<PanicHook>,
}

impl CatchPanicLayer {
    /// The layer with no application hook: log line + JSON 500.
    pub fn new() -> Self {
        Self { hook: None }
    }

    /// The layer with an application hook invoked once per caught panic.
    pub fn with_hook(hook: Option<PanicHook>) -> Self {
        Self { hook }
    }
}

impl<S> Layer<S> for CatchPanicLayer {
    type Service = CatchPanic<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CatchPanic {
            inner,
            hook: self.hook.clone(),
        }
    }
}

/// Catches an unwind from the inner service — raised synchronously in `call`
/// or later while polling its future — and answers a JSON 500.
#[derive(Clone)]
pub struct CatchPanic<S> {
    inner: S,
    hook: Option<PanicHook>,
}

impl<S, ReqBody> Service<Request<ReqBody>> for CatchPanic<S>
where
    S: Service<Request<ReqBody>, Response = Response>,
{
    type Response = Response;
    type Error = S::Error;
    type Future = CatchPanicFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        // An `Arc<str>` bump, not a copy: `MatchedPath` is the route template
        // the router already interned. `None` at the outermost install slot,
        // where routing has not happened yet from this layer's point of view.
        let route = req.extensions().get::<MatchedPath>().cloned();

        match std::panic::catch_unwind(AssertUnwindSafe(|| self.inner.call(req))) {
            Ok(future) => CatchPanicFuture {
                inner: Some(future),
                response: None,
                route,
                hook: self.hook.clone(),
            },
            Err(payload) => CatchPanicFuture {
                inner: None,
                response: Some(handle_panic(payload, route.as_ref(), self.hook.as_ref())),
                route: None,
                hook: None,
            },
        }
    }
}

pin_project! {
    /// Response future of [`CatchPanic`].
    ///
    /// `inner` is `None` once the panic has already been turned into a
    /// response — either by a synchronous unwind out of `call`, or by an
    /// unwind out of an earlier poll.
    pub struct CatchPanicFuture<F> {
        #[pin]
        inner: Option<F>,
        response: Option<Response>,
        route: Option<MatchedPath>,
        hook: Option<PanicHook>,
    }
}

impl<F, E> Future for CatchPanicFuture<F>
where
    F: Future<Output = Result<Response, E>>,
{
    type Output = Result<Response, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        // The synchronous-unwind path: the 500 was built in `call`.
        if let Some(response) = this.response.take() {
            return Poll::Ready(Ok(response));
        }

        let Some(inner) = this.inner.as_mut().as_pin_mut() else {
            panic!("CatchPanicFuture polled after completion");
        };

        match std::panic::catch_unwind(AssertUnwindSafe(|| inner.poll(cx))) {
            Ok(poll) => poll,
            Err(payload) => {
                // Drop the panicked future before anything else: it unwound
                // mid-poll, so its state is unknown and it must never be
                // polled again.
                this.inner.set(None);
                Poll::Ready(Ok(handle_panic(
                    payload,
                    this.route.as_ref(),
                    this.hook.as_ref(),
                )))
            }
        }
    }
}
