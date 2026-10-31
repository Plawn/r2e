// The tracing entry points moved to `runtime::tracing` (stack in the
// `r2e-tracing` crate); re-exported here so `runtime::layers::*` keeps its paths.
pub use crate::runtime::tracing::{
    init_tracing, init_tracing_from_config, init_tracing_with_config, resolve_tracing_config,
    try_init_tracing_with_config, warn_if_output_differs, SubscriberAlreadyInstalled,
};
pub use r2e_tracing::UpstreamNoise;

// The tower-http-backed pieces live in `r2e_http::layers` (the one crate
// allowed to name tower-http); re-exported here so `runtime::layers::*` and
// the crate-root `default_cors` keep their paths.
pub use crate::http::layers::{default_cors, normalize_path_router};

/// Returns the layer that converts panics into 500 responses in the
/// application's error envelope (`projector`), logs one structured `error`
/// event, and invokes the application hook from
/// [`AppBuilder::on_panic`](crate::builder::AppBuilder::on_panic).
///
/// Installed by `build_inner` alone (twice — see [`crate::runtime::panic`]
/// for why the primary slot is the *innermost* one); there is no plugin for it.
pub fn catch_panic_layer_with(
    hook: Option<crate::runtime::panic::PanicHook>,
    projector: crate::error::ErrorProjector,
) -> crate::runtime::panic::CatchPanicLayer {
    crate::runtime::panic::CatchPanicLayer::with(hook, projector)
}

/// Install the framework's 404 and 405 responders on a fully assembled
/// router, both answering through the application's error envelope.
///
/// - **404** — only when nothing else claimed the router fallback
///   ([`Routes::has_custom_fallback`](crate::http::Routes::has_custom_fallback)):
///   a controller `#[fallback]`, a router merged with its own `.fallback(..)`,
///   or a plugin such as the SPA fallback keep winning. Without one, axum
///   would answer an empty-bodied 404; R2E answers `Rejection::not_found`
///   projected through the envelope (`{"error":"Not found"}` by default).
/// - **405** — the per-route method-not-allowed fallback, for every route
///   still on axum's default (a route's own custom fallback is kept). axum
///   adds the `Allow` header afterwards. `RejectionKind::MethodNotAllowed`
///   projected through the envelope (`{"error":"Method not allowed"}` by
///   default) replaces axum's bare, bodiless 405.
///
/// Must run after every route, controller fallback and Routes-stage plugin
/// router has been merged: routes added later would miss the 405 responder,
/// and a fallback merged later would panic against the one installed here.
pub fn framework_fallbacks(
    app: crate::http::Routes,
    projector: &crate::error::ErrorProjector,
) -> crate::http::Routes {
    use crate::error::{Rejection, RejectionKind};

    let mut app = app;
    if !app.has_custom_fallback() {
        let projector = projector.clone();
        app = app.fallback(move |_: (), _: crate::http::Request| {
            let projector = projector.clone();
            async move { projector.project(Rejection::not_found("Not found")) }
        });
    }
    let projector = projector.clone();
    app.method_not_allowed_fallback(move |_: (), _: crate::http::Request| {
        let projector = projector.clone();
        async move {
            projector.project(Rejection::new(
                RejectionKind::MethodNotAllowed,
                "Method not allowed",
            ))
        }
    })
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
