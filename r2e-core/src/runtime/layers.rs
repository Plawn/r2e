use crate::runtime::tracing_config::{LogFormat, TracingConfig};
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::EnvFilter;

/// The configuration the process-global subscriber was actually installed
/// with, when R2E installed it.
///
/// A `tracing` subscriber is a one-shot process global: the first install
/// wins and every later one is a no-op. Recording what won lets a losing
/// caller tell the two cases apart — re-installing the *same* knobs (an
/// entry point and a plugin both reading the same `tracing:` section, or
/// `#[r2e::test]` calling [`init_tracing`] once per test) is redundant but
/// harmless, whereas losing with *different* knobs means the output the
/// caller asked for is silently not the output the process produces.
static INSTALLED: std::sync::OnceLock<TracingConfig> = std::sync::OnceLock::new();

/// An init that lost the race to a subscriber installed earlier.
///
/// `installed` carries the winning configuration when R2E installed it, and
/// is `None` when the subscriber came from elsewhere (the application, a test
/// harness, another library).
#[derive(Debug, Clone)]
pub struct SubscriberAlreadyInstalled {
    /// The configuration that actually won, when it is known.
    pub installed: Option<TracingConfig>,
}

impl SubscriberAlreadyInstalled {
    /// Whether the winning subscriber differs from what `requested` asked
    /// for — i.e. whether losing the race actually changed the output.
    ///
    /// An unknown winner counts as different: nothing says it honours the
    /// requested format or filter.
    pub fn changes_output(&self, requested: &TracingConfig) -> bool {
        self.installed.as_ref() != Some(requested)
    }
}

impl std::fmt::Display for SubscriberAlreadyInstalled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.installed {
            Some(c) => write!(
                f,
                "a global tracing subscriber is already installed (format={:?}, filter={:?})",
                c.effective_format(),
                c.filter
            ),
            None => write!(f, "a global tracing subscriber is already installed"),
        }
    }
}

impl std::error::Error for SubscriberAlreadyInstalled {}

/// Initialise the global `tracing` subscriber with a standard `fmt` layer.
///
/// Respects the `RUST_LOG` environment variable. Falls back to the
/// [`TracingConfig`] default filter (`info`) when `RUST_LOG` is not set.
///
/// This function is idempotent — calling it more than once is safe (subsequent
/// calls are silently ignored). It is called automatically by the [`Tracing`]
/// plugin, so you only need to call it manually if you want logs *before* the
/// plugin is installed (e.g. during state construction).
///
/// It installs the **built-in defaults**, ignoring the application's
/// `tracing:` section; an entry point that should honour that section calls
/// [`init_tracing_from_config`] instead.
///
/// [`Tracing`]: crate::builtins::Tracing
pub fn init_tracing() {
    init_tracing_with_config(&TracingConfig::default());
}

/// Initialise the global `tracing` subscriber from the application's own
/// configuration — this is what R2E's entry points call.
///
/// Loads `application.yaml` (profile overlay and `R2E_*` env overlay
/// included) and installs its `tracing:` section, so an app that declares
/// `format: json` gets JSON from its very first log line, without having to
/// install a subscriber itself.
///
/// A missing configuration file is normal and silent. An unreadable or
/// malformed one falls back to the built-in defaults and warns — through the
/// subscriber it just installed, so the message is visible — rather than
/// failing: a bad config file must never cost the app the log line that
/// explains the boot error that follows.
pub fn init_tracing_from_config() {
    let (config, problem) = resolve_tracing_config();

    if let Err(lost) = try_init_tracing_with_config(&config) {
        warn_if_output_differs(&lost, &config);
    }
    if let Some(problem) = problem {
        tracing::warn!("r2e: falling back to the built-in tracing defaults — {problem}");
    }
}

/// The configuration [`init_tracing_from_config`] installs, plus the reason it
/// fell back to the built-in defaults when it did.
///
/// Split out of the install so the *resolution* — the part that reads files —
/// can be exercised on its own; installing is a one-shot process global and
/// can be observed only once per process.
#[doc(hidden)]
pub fn resolve_tracing_config() -> (TracingConfig, Option<String>) {
    match crate::config::R2eConfig::load() {
        Ok(loaded) => {
            use crate::config::ConfigProperties;
            match TracingConfig::from_config(&loaded, Some("tracing")) {
                Ok(config) => (config, None),
                Err(e) => (
                    TracingConfig::default(),
                    Some(format!("the `tracing:` section is invalid: {e}")),
                ),
            }
        }
        Err(e) => (
            TracingConfig::default(),
            Some(format!("the configuration could not be loaded: {e}")),
        ),
    }
}

/// Warn when an init lost the race to a subscriber that logs differently.
///
/// Losing with the very same configuration is the common, harmless case (an
/// entry point and a plugin reading the same section); staying quiet there is
/// what makes the warning worth reading when it does appear.
pub fn warn_if_output_differs(lost: &SubscriberAlreadyInstalled, requested: &TracingConfig) {
    if lost.changes_output(requested) {
        tracing::warn!(
            "r2e: this tracing configuration (format={:?}, filter={:?}) is ignored — {lost}. \
             Install the subscriber before R2E does (in `App::setup`), or opt out of R2E's with \
             `app_main!(MyApp, tracing = false)`.",
            requested.effective_format(),
            requested.filter,
        );
    }
}

/// Initialise the global `tracing` subscriber from a [`TracingConfig`].
///
/// `RUST_LOG` env var always takes priority over `config.filter`.
/// This function is idempotent — subsequent calls are silently ignored. Use
/// [`try_init_tracing_with_config`] to find out whether this call is the one
/// that took effect.
pub fn init_tracing_with_config(config: &TracingConfig) {
    let _ = try_init_tracing_with_config(config);
}

/// [`init_tracing_with_config`], reporting whether it won the race.
///
/// `Err(SubscriberAlreadyInstalled)` means a subscriber was already in place
/// and this configuration had no effect at all — the case that used to be a
/// silent no-op.
pub fn try_init_tracing_with_config(
    config: &TracingConfig,
) -> Result<(), SubscriberAlreadyInstalled> {
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

    let outcome = match config.effective_format() {
        LogFormat::Json => tracing_subscriber::fmt()
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
            .try_init(),
        LogFormat::Pretty => tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_target(target)
            .with_thread_ids(thread_ids)
            .with_thread_names(thread_names)
            .with_file(file)
            .with_line_number(line_number)
            .with_level(level)
            .with_ansi(ansi)
            .with_span_events(span_events)
            .try_init(),
    };

    match outcome {
        Ok(()) => {
            // Best-effort: the race this loses is another thread installing
            // at the same instant, and then it is that one's config that is
            // recorded — which is exactly what we want to remember.
            let _ = INSTALLED.set(config.clone());
            Ok(())
        }
        Err(_) => Err(SubscriberAlreadyInstalled {
            installed: INSTALLED.get().cloned(),
        }),
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

/// Returns the layer that converts panics into JSON 500 responses, logs one
/// structured `error` event, and invokes the application panic hook.
///
/// See [`crate::runtime::panic`] for why the primary install slot is the
/// *innermost* one rather than the outermost.
pub fn catch_panic_layer() -> crate::runtime::panic::CatchPanicLayer {
    crate::runtime::panic::CatchPanicLayer::new()
}

/// Same, with the application hook from
/// [`AppBuilder::on_panic`](crate::builder::AppBuilder::on_panic).
pub fn catch_panic_layer_with(
    hook: Option<crate::runtime::panic::PanicHook>,
) -> crate::runtime::panic::CatchPanicLayer {
    crate::runtime::panic::CatchPanicLayer::with_hook(hook)
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
