//! The `HttpTrace` plugin — one span + one summary line per HTTP request.
//!
//! The layer itself lives in [`crate::runtime::http_trace`]; this module owns
//! the plugin, its builder, and the typed `trace:` config section.

use std::sync::Arc;

use crate::plugin::{Plugin, PluginBuildContext, PluginBuildError};
use crate::runtime::http_trace::{HttpTraceLayer, HttpTraceSettings, MakeRequestSpan};

/// Typed configuration for the [`HttpTrace`] plugin, read from the `trace.*`
/// YAML section.
///
/// `tracing:` says **where logs go** (format, filter, ansi — the subscriber);
/// `trace:` says **what each request logs**.
///
/// ```yaml
/// trace:
///   enabled: true
///   exclude-paths: ["/health", "/metrics"]   # prefix match, raw path OR route label
///   request-id: true                         # read x-request-id or mint a UUID, echo it back
///   record-path: false                       # raw path on the summary event (never on the span)
///   record-query: false
///   capture-headers: []                      # recorded on the span
///   summary: true                            # one INFO line per request
///   request-event: false                     # opt-in DEBUG "request started" line
/// ```
///
/// Every field is optional. Precedence per knob: **explicit builder setting >
/// this file section > [preset](HttpTrace::preset) > built-in default**.
#[derive(r2e_macros::ConfigProperties, Clone, Debug, Default)]
pub struct HttpTraceConfig {
    /// Path prefixes excluded from tracing entirely — no span, no summary
    /// event, no request id. Matched against the raw request path **and** the
    /// bounded route label, like `prometheus.exclude-paths`.
    ///
    /// Default: `["/health", "/metrics"]` — probe and scrape traffic is the
    /// number-one reason request logging gets turned off. An app that mounts
    /// something it *does* want traced under those prefixes sets
    /// `exclude-paths: []`.
    #[config(key = "exclude-paths")]
    pub exclude_paths: Option<Vec<String>>,

    /// Resolve a request id (inbound `x-request-id`, else a fresh UUID v4),
    /// record it on the span and echo it on the response. Default `true`.
    #[config(key = "request-id")]
    pub request_id: Option<bool>,

    /// Add the raw request path to the summary **event**. Default `false` —
    /// paths carry tokens and ids, and the span is deliberately limited to the
    /// bounded route template so a secret never propagates to every handler
    /// log line.
    #[config(key = "record-path")]
    pub record_path: Option<bool>,

    /// Add the raw query string to the summary **event**. Default `false`,
    /// same reasoning as `record-path`.
    #[config(key = "record-query")]
    pub record_query: Option<bool>,

    /// Inbound header names recorded on the span. Validated at boot: an
    /// invalid header name aborts startup rather than failing silently per
    /// request. Default: none.
    #[config(key = "capture-headers")]
    pub capture_headers: Option<Vec<String>>,

    /// Emit the one-line `request completed` summary (INFO below 500, ERROR at
    /// 5xx). Default `true`.
    pub summary: Option<bool>,

    /// Also emit a `request started` line at DEBUG. Default `false`.
    #[config(key = "request-event")]
    pub request_event: Option<bool>,
}

/// Per-request HTTP tracing: one span, one summary line, request ids,
/// exclusions.
///
/// ```ignore
/// AppBuilder::new()
///     .plugin(HttpTrace::new())                   // sane defaults
///     .plugin(HttpTrace::builder()
///         .exclude_path("/internal")
///         .capture_header("user-agent")
///         .record_path(true)                      // opt IN to the raw path
///         .build())
/// ```
///
/// # What it emits
///
/// A span named `request` at INFO on target `r2e::http`, entered for the whole
/// handler future — so `request_id` and `route` decorate every line the handler
/// logs — carrying `method`, `route` (the **bounded route template**, never the
/// raw path), `request_id`, and the configured `capture-headers`. Then one
/// summary event inside that span:
///
/// ```text
/// INFO  r2e::http: request completed status=200 latency_ms=3.2
/// ERROR r2e::http: request completed status=503 latency_ms=12.0
/// ```
///
/// `latency_ms` is measured to the response **head**; streaming bodies are not
/// included (which is why the Prometheus layer keeps its own timer).
///
/// # Relationship to the other plugins
///
/// | Plugin | Owns |
/// |---|---|
/// | [`Tracing`](crate::builtins::Tracing) / [`ConfiguredTracing`](crate::builtins::ConfiguredTracing) | the **subscriber** (format, filter, ansi). No HTTP layer. |
/// | `HttpTrace` | the per-request **span**, summary event, request id, exclusions |
/// | `r2e_observability::Observability` | OTLP export + propagation — installs *this* layer with an OpenTelemetry span shape, so there is still exactly one span per request |
/// | [`RequestIdPlugin`](crate::builtins::request_id::RequestIdPlugin) | `x-request-id` only; `HttpTrace` already does it, and installing both in either order is harmless |
///
/// Under `r2e::launch` / `#[r2e::main]` / `#[r2e::test]` the subscriber is
/// installed by the entry point from the app's own `tracing:` section, so most
/// applications install `HttpTrace` alone.
///
/// # Configuration
///
/// [`HttpTraceConfig`], section `trace`, gate `trace.enabled` (disabled = no
/// layer at all: no span, no event, no request id).
pub struct HttpTrace {
    builder: HttpTraceConfig,
    preset: Option<HttpTraceConfig>,
    make_span: Option<Arc<dyn MakeRequestSpan>>,
}

impl Default for HttpTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpTrace {
    /// The plugin with built-in defaults (see [`HttpTraceConfig`]), still
    /// overridable from the app's `trace:` section.
    #[must_use]
    pub fn new() -> Self {
        Self {
            builder: HttpTraceConfig::default(),
            preset: None,
            make_span: None,
        }
    }

    /// Start building an explicitly configured plugin.
    #[must_use]
    pub fn builder() -> HttpTraceBuilder {
        HttpTraceBuilder::default()
    }

    /// A full [`HttpTraceConfig`] filling the **default** slot of the
    /// precedence chain: explicit builder knob > app file > *preset* >
    /// built-in default.
    ///
    /// This is the "shared company baseline" lane: a baseline crate ships the
    /// house HTTP-log contract here, and every service's own
    /// `application.yaml` still has the last word without touching code.
    /// Plain builder methods would be backwards for that job — a builder knob
    /// beats the file, so the baseline would silently override every app.
    ///
    /// ```ignore
    /// // company crate `acme-baseline`
    /// pub fn http_trace() -> HttpTrace {
    ///     HttpTrace::preset(HttpTraceConfig {
    ///         exclude_paths: Some(vec!["/health".into(), "/metrics".into(), "/docs".into()]),
    ///         capture_headers: Some(vec!["x-tenant".into()]),
    ///         ..Default::default()
    ///     })
    /// }
    ///
    /// #[module(plugins(HttpTrace = acme_baseline::http_trace()))]
    /// pub struct Baseline;
    /// ```
    #[must_use]
    pub fn preset(config: HttpTraceConfig) -> Self {
        Self {
            builder: HttpTraceConfig::default(),
            preset: Some(config),
            make_span: None,
        }
    }
}

/// Builder for [`HttpTrace`]. Every setting here takes precedence over the
/// app's `trace:` section — it is the "I mean exactly this" lane.
#[derive(Default)]
pub struct HttpTraceBuilder {
    config: HttpTraceConfig,
    preset: Option<HttpTraceConfig>,
    make_span: Option<Arc<dyn MakeRequestSpan>>,
}

impl HttpTraceBuilder {
    /// Exclude one path prefix from tracing (repeatable).
    ///
    /// Calling this **replaces** the default `["/health", "/metrics"]` with
    /// exactly what you list.
    #[must_use]
    pub fn exclude_path(mut self, prefix: impl Into<String>) -> Self {
        self.config
            .exclude_paths
            .get_or_insert_with(Vec::new)
            .push(prefix.into());
        self
    }

    /// Replace the excluded path prefixes wholesale (`[]` traces everything).
    #[must_use]
    pub fn exclude_paths<I, S>(mut self, prefixes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.exclude_paths = Some(prefixes.into_iter().map(Into::into).collect());
        self
    }

    /// Resolve/echo a request id (default `true`).
    #[must_use]
    pub fn request_id(mut self, enabled: bool) -> Self {
        self.config.request_id = Some(enabled);
        self
    }

    /// Put the raw request path on the summary event (default `false`).
    #[must_use]
    pub fn record_path(mut self, enabled: bool) -> Self {
        self.config.record_path = Some(enabled);
        self
    }

    /// Put the raw query string on the summary event (default `false`).
    #[must_use]
    pub fn record_query(mut self, enabled: bool) -> Self {
        self.config.record_query = Some(enabled);
        self
    }

    /// Record one inbound header on the span (repeatable).
    #[must_use]
    pub fn capture_header(mut self, name: impl Into<String>) -> Self {
        self.config
            .capture_headers
            .get_or_insert_with(Vec::new)
            .push(name.into());
        self
    }

    /// Replace the captured header list wholesale.
    #[must_use]
    pub fn capture_headers<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.capture_headers = Some(names.into_iter().map(Into::into).collect());
        self
    }

    /// Emit the `request completed` summary event (default `true`).
    #[must_use]
    pub fn summary(mut self, enabled: bool) -> Self {
        self.config.summary = Some(enabled);
        self
    }

    /// Also emit a `request started` event at DEBUG (default `false`).
    #[must_use]
    pub fn request_event(mut self, enabled: bool) -> Self {
        self.config.request_event = Some(enabled);
        self
    }

    /// Replace the span shape. Everything else — exclusions, request id,
    /// timing, the summary event — stays with the layer.
    #[must_use]
    pub fn make_span<M: MakeRequestSpan>(mut self, make_span: M) -> Self {
        self.make_span = Some(Arc::new(make_span));
        self
    }

    /// Defaults contributed by a shared baseline — see [`HttpTrace::preset`].
    #[must_use]
    pub fn preset(mut self, config: HttpTraceConfig) -> Self {
        self.preset = Some(config);
        self
    }

    /// Finish the plugin.
    #[must_use]
    pub fn build(self) -> HttpTrace {
        HttpTrace {
            builder: self.config,
            preset: self.preset,
            make_span: self.make_span,
        }
    }
}

/// The built-in `exclude-paths` default.
pub const DEFAULT_EXCLUDE_PATHS: [&str; 2] = ["/health", "/metrics"];

/// Merge the three configuration sources into the effective
/// [`HttpTraceSettings`].
///
/// Precedence per knob: `builder` > `file` > `preset` > built-in default.
/// Header names are validated here so an invalid one is a boot error rather
/// than a silent per-request skip.
///
/// Exposed (hidden) so the precedence contract can be unit-tested.
#[doc(hidden)]
pub fn resolve_settings(
    builder: HttpTraceConfig,
    file: Option<HttpTraceConfig>,
    preset: Option<HttpTraceConfig>,
) -> Result<HttpTraceSettings, PluginBuildError> {
    let file = file.unwrap_or_default();
    let preset = preset.unwrap_or_default();

    let exclude_paths = builder
        .exclude_paths
        .or(file.exclude_paths)
        .or(preset.exclude_paths)
        .unwrap_or_else(|| {
            DEFAULT_EXCLUDE_PATHS
                .iter()
                .map(|p| (*p).to_string())
                .collect()
        });

    let capture_headers = builder
        .capture_headers
        .or(file.capture_headers)
        .or(preset.capture_headers)
        .unwrap_or_default();
    let capture_headers = capture_headers
        .into_iter()
        .map(|name| {
            crate::http::HeaderName::try_from(name.as_str()).map_err(|_| -> PluginBuildError {
                format!("`trace.capture-headers` contains an invalid header name: {name:?}").into()
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(HttpTraceSettings {
        exclude_paths: Arc::from(exclude_paths),
        request_id: builder
            .request_id
            .or(file.request_id)
            .or(preset.request_id)
            .unwrap_or(true),
        record_path: builder
            .record_path
            .or(file.record_path)
            .or(preset.record_path)
            .unwrap_or(false),
        record_query: builder
            .record_query
            .or(file.record_query)
            .or(preset.record_query)
            .unwrap_or(false),
        capture_headers: Arc::from(capture_headers),
        summary: builder
            .summary
            .or(file.summary)
            .or(preset.summary)
            .unwrap_or(true),
        request_event: builder
            .request_event
            .or(file.request_event)
            .or(preset.request_event)
            .unwrap_or(false),
    })
}

impl Plugin for HttpTrace {
    type Provided = ();
    type Deps = ();
    type Config = HttpTraceConfig;
    type Controllers = ();
    const CONFIG_PREFIX: Option<&'static str> = Some("trace");

    async fn build(
        self,
        _deps: Self::Deps,
        config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        // Resolved (and header names validated) even when disabled: a typo in
        // `trace.capture-headers` should not lie dormant until someone flips
        // `trace.enabled` back on in production.
        let settings = resolve_settings(self.builder, config, self.preset)?;

        if !ctx.enabled() {
            tracing::debug!(
                "HttpTrace disabled via `trace.enabled = false`; no request span, \
                 no summary event, no request id"
            );
            return Ok(());
        }

        let make_span = self.make_span;
        ctx.add_layer(move |router| match make_span {
            Some(make_span) => router.layer(HttpTraceLayer::from_shared(settings, make_span)),
            None => router.layer(HttpTraceLayer::new(settings)),
        });
        Ok(())
    }
}
