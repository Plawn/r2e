use crate::http::StatusCode;
use crate::runtime::tracing_config::{LogFormat, TracingConfig};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

/// Initialise the global `tracing` subscriber with a standard `fmt` layer.
///
/// Respects the `RUST_LOG` environment variable. Falls back to
/// `info,tower_http=debug` when `RUST_LOG` is not set.
///
/// This function is idempotent — calling it more than once is safe (subsequent
/// calls are silently ignored). It is called automatically by the [`Tracing`]
/// plugin, so you only need to call it manually if you want logs *before* the
/// plugin is installed (e.g. during state construction).
///
/// [`Tracing`]: crate::builtins::Tracing
pub fn init_tracing() {
    init_tracing_with_config(&TracingConfig::default());
}

/// Initialise the global `tracing` subscriber from a [`TracingConfig`].
///
/// `RUST_LOG` env var always takes priority over `config.filter`.
/// This function is idempotent — subsequent calls are silently ignored.
pub fn init_tracing_with_config(config: &TracingConfig) {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| config.filter.parse().unwrap());

    let span_events = config.effective_span_events();
    let target = config.target.unwrap_or(true);
    let thread_ids = config.thread_ids.unwrap_or(false);
    let thread_names = config.thread_names.unwrap_or(false);
    let file = config.file.unwrap_or(false);
    let line_number = config.line_number.unwrap_or(false);
    let level = config.level.unwrap_or(true);
    let ansi = config.ansi.unwrap_or(true);

    match config.effective_format() {
        LogFormat::Json => {
            let _ = tracing_subscriber::fmt()
                .json()
                .with_env_filter(env_filter)
                .with_target(target)
                .with_thread_ids(thread_ids)
                .with_thread_names(thread_names)
                .with_file(file)
                .with_line_number(line_number)
                .with_level(level)
                .with_ansi(ansi)
                .with_span_events(span_events)
                .try_init();
        }
        LogFormat::Pretty => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_target(target)
                .with_thread_ids(thread_ids)
                .with_thread_names(thread_names)
                .with_file(file)
                .with_line_number(line_number)
                .with_level(level)
                .with_ansi(ansi)
                .with_span_events(span_events)
                .try_init();
        }
    }
}

/// Returns a permissive CORS layer that allows any origin, method, and headers.
///
/// Suitable for development or internal services. For production, prefer
/// `AppBuilder::with_cors_config` with a stricter `CorsLayer`.
pub fn default_cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
}

/// Returns a `TraceLayer` configured for HTTP request/response tracing.
///
/// Uses `tower_http`'s default classification which logs at the `DEBUG` level
/// for requests and responses.
pub fn default_trace(
) -> TraceLayer<tower_http::classify::SharedClassifier<tower_http::classify::ServerErrorsAsFailures>>
{
    TraceLayer::new_for_http()
}

/// Returns a `CatchPanicLayer` that converts panics into JSON 500 responses.
pub fn catch_panic_layer(
) -> CatchPanicLayer<fn(Box<dyn std::any::Any + Send>) -> crate::http::Response> {
    CatchPanicLayer::custom(panic_handler as fn(_) -> _)
}

/// Wrap a fully-built router in pre-routing trailing-slash normalization.
///
/// A middleware added via `Router::layer` runs after routing and cannot
/// change which route matches, so the rewrite must wrap the router itself:
/// the whole router goes inside tower-http's `NormalizePath` service, and
/// the wrapped service is re-embedded as the fallback of a fresh routerless
/// `Router` so callers keep receiving a plain `Router`. The outer router is
/// a trivial extra dispatch hop — the meaningful routing (and `MatchedPath`
/// insertion) happens once, in the wrapped inner router. Anything applied
/// OUTSIDE this wrap (e.g. `router_wraps`) therefore never sees
/// `MatchedPath`.
///
/// tower-http's `trim_trailing_slash` also collapses a leading run of
/// slashes (`//admin` → `/admin`); see the `NormalizePath` plugin docs.
pub fn normalize_path_router(router: crate::http::Router) -> crate::http::Router {
    use tower::Layer as _;
    let svc = tower_http::normalize_path::NormalizePathLayer::trim_trailing_slash().layer(router);
    crate::http::Router::new().fallback_service(svc)
}

/// A pass-through middleware whose only job is to **own** the resolved bean
/// graph for as long as anything derived from the router can still reach it.
///
/// The graph cannot be owned from the inside: beans hold a
/// [`GraphHandle`](crate::plugin::GraphHandle) (per-tenant maps, resource
/// factories), and that handle is weak precisely so
/// `BeanContext → bean → handle → BeanContext` is not an unbreakable cycle.
/// Something outside the graph must therefore keep it alive, and the router is
/// the honest owner: it is what turns a request into bean access.
///
/// # Ownership follows the request, not just the service
///
/// Holding the `Arc` in the service value alone is **not** enough. A caller may
/// legitimately drop the service the instant it has a future —
/// `tower::ServiceExt::oneshot` does exactly that: it replaces the service with
/// its future before ever polling it, so `router.oneshot(req)` on the last
/// router clone would drop the graph *before* the handler runs, and every
/// `GraphHandle` upgrade inside it would return `None`. Hyper is equally free
/// to drop the connection service while a response body is still streaming.
///
/// So the `Arc` is cloned into each request: [`GraphKeepAliveFuture`] carries it
/// alongside the inner future, then hands it to the response **body**, which
/// keeps it alive until the last frame has been produced. The guarantee is:
/// *the graph outlives every request future and every response body derived
/// from this router*, plus the router itself.
///
/// Installed once in `build_inner`, so every entry point that produces a
/// router — `build()`, `build_with_consumers()`, `prepare()`/`serve()` — keeps
/// its graph, and dropping the app (and everything in flight) drops the graph,
/// with it every bean, pool and per-tenant resource. It changes neither the
/// request nor the response — only the body's *type*, which is already the
/// erased `Body`.
#[derive(Clone)]
pub(crate) struct GraphKeepAlive<S> {
    inner: S,
    graph: std::sync::Arc<crate::beans::BeanContext>,
}

pin_project_lite::pin_project! {
    /// The future returned by [`GraphKeepAlive`]: the inner future plus a
    /// strong reference to the graph, moved into the response body on
    /// completion. See the ownership note on [`GraphKeepAlive`].
    pub(crate) struct GraphKeepAliveFuture<F> {
        #[pin]
        inner: F,
        // `Option` only so it can be moved out on completion; it is `Some` for
        // the whole polling life of the future.
        graph: Option<std::sync::Arc<crate::beans::BeanContext>>,
    }
}

impl<F, E> std::future::Future for GraphKeepAliveFuture<F>
where
    F: std::future::Future<Output = Result<crate::http::Response, E>>,
{
    type Output = Result<crate::http::Response, E>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        use std::task::Poll;

        let this = self.project();
        let response = match this.inner.poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Ok(response)) => response,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
        };
        // MUST hold: `graph` is `Some` on the completing poll — it is set in
        // `call` and taken exactly here, and a future must not be polled after
        // it returned `Ready`. If it ever were, the body simply stops carrying
        // the graph (the request already completed), never a panic.
        let Some(graph) = this.graph.take() else {
            return Poll::Ready(Ok(response));
        };
        // The response body outlives this future — hyper splits the response
        // into head and body and may stream the body long after the service
        // (and its future) are gone. Response *extensions* would not do: they
        // travel with the head, which hyper drops independently. So the `Arc`
        // rides inside the body itself.
        Poll::Ready(Ok(response.map(move |body| {
            crate::http::Body::new(GraphBody { inner: body, graph })
        })))
    }
}

pin_project_lite::pin_project! {
    /// The response body, carrying a strong reference to the bean graph.
    ///
    /// Every method delegates to the inner body — including `size_hint` and
    /// `is_end_stream`, which is the whole reason this is not
    /// `BodyExt::map_frame`: that combinator cannot know the mapped frames keep
    /// their sizes, so it reports an unknown length and hyper falls back to
    /// chunked transfer encoding for EVERY response. This wrapper changes no
    /// byte and no length; the only thing it adds is the `Arc` it drops when
    /// the body does.
    pub(crate) struct GraphBody<B> {
        #[pin]
        inner: B,
        // Read by nothing: its lifetime IS the payload.
        graph: std::sync::Arc<crate::beans::BeanContext>,
    }
}

impl<B: http_body::Body> http_body::Body for GraphBody<B> {
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        self.project().inner.poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

impl<S, R> tower::Service<R> for GraphKeepAlive<S>
where
    S: tower::Service<R, Response = crate::http::Response>,
{
    type Response = crate::http::Response;
    type Error = S::Error;
    type Future = GraphKeepAliveFuture<S::Future>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: R) -> Self::Future {
        GraphKeepAliveFuture {
            inner: self.inner.call(req),
            graph: Some(std::sync::Arc::clone(&self.graph)),
        }
    }
}

/// The [`tower::Layer`] installing [`GraphKeepAlive`].
pub(crate) fn graph_keep_alive<S>(
    graph: std::sync::Arc<crate::beans::BeanContext>,
) -> impl tower::Layer<S, Service = GraphKeepAlive<S>> + Clone + Send + Sync + 'static {
    tower::layer::layer_fn(move |inner| GraphKeepAlive {
        inner,
        graph: std::sync::Arc::clone(&graph),
    })
}

fn panic_handler(_err: Box<dyn std::any::Any + Send>) -> crate::http::Response {
    crate::http::response::static_json(
        StatusCode::INTERNAL_SERVER_ERROR,
        r#"{"error":"Internal server error"}"#,
    )
}
