use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError, RwLock};

use opentelemetry::trace::{SpanBuilder, TraceContextExt, Tracer, TracerProvider};
use opentelemetry_sdk::trace::{
    Sampler, SdkTracer, SdkTracerProvider, SpanExporter, TracerProviderBuilder,
};
use opentelemetry_sdk::Resource;
use r2e_core::{EnrichContext, FieldEnricher};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

use crate::config::{ObservabilityConfig, OtlpProtocol};

/// Start OpenTelemetry export and wire it into R2E's subscriber.
///
/// Builds the tracer provider (resource, sampler, OTLP/HTTP exporter) and
/// **contributes** to the subscriber R2E installed — it never builds one:
///
/// - the `tracing-opentelemetry` layer goes into the late-layer slot
///   ([`r2e_core::contribute_layer`]), so spans are exported and
///   `Span::current().context()` works;
/// - a field enricher ([`r2e_core::add_enricher`]) adds `trace_id` and
///   `span_id` to every console line emitted inside a traced span.
///
/// Both are process-wide and contributed once: a later call (a second app
/// boot in the same process, a dev-reload cycle) only points the contributed
/// layer at the new provider. When the installed subscriber is a hand-built
/// one without [`r2e_core::late_layers`], nothing is exported and one `WARN`
/// says so.
///
/// Returns a guard that flushes traces on drop.
pub fn init_tracing(config: &ObservabilityConfig) -> OtelGuard {
    let provider_builder = provider_builder(config);

    #[cfg(feature = "otlp")]
    let provider_builder = {
        use opentelemetry_otlp::WithExportConfig;
        let endpoint = normalized_otlp_traces_endpoint(&config.otlp_endpoint);
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(endpoint)
            .build()
            .expect("Failed to build OTLP span exporter");
        provider_builder.with_batch_exporter(exporter)
    };

    finish(config, provider_builder)
}

/// [`init_tracing`] with `exporter` (synchronous: one export per span end)
/// in place of the OTLP one — the seam tests use to read exported spans.
#[doc(hidden)]
pub fn init_tracing_with_exporter<E: SpanExporter + 'static>(
    config: &ObservabilityConfig,
    exporter: E,
) -> OtelGuard {
    finish(
        config,
        provider_builder(config).with_simple_exporter(exporter),
    )
}

/// Resource and sampler, shared by both entry points.
fn provider_builder(config: &ObservabilityConfig) -> TracerProviderBuilder {
    let mut resource_kv = vec![opentelemetry::KeyValue::new(
        opentelemetry_semantic_conventions::attribute::SERVICE_NAME,
        config.service_name.clone(),
    )];
    if let Some(ref version) = config.service_version {
        resource_kv.push(opentelemetry::KeyValue::new(
            opentelemetry_semantic_conventions::attribute::SERVICE_VERSION,
            version.clone(),
        ));
    }
    for (k, v) in &config.resource_attributes {
        resource_kv.push(opentelemetry::KeyValue::new(k.clone(), v.clone()));
    }
    let resource = Resource::builder().with_attributes(resource_kv).build();

    let sampler = if config.sampling_ratio >= 1.0 {
        Sampler::AlwaysOn
    } else if config.sampling_ratio <= 0.0 {
        Sampler::AlwaysOff
    } else {
        Sampler::TraceIdRatioBased(config.sampling_ratio)
    };

    SdkTracerProvider::builder()
        .with_sampler(sampler)
        .with_resource(resource)
}

fn finish(config: &ObservabilityConfig, provider_builder: TracerProviderBuilder) -> OtelGuard {
    let provider = provider_builder.build();
    *CURRENT_TRACER
        .write()
        .unwrap_or_else(PoisonError::into_inner) = Some(provider.tracer("r2e"));
    contribute_once();

    if config.otlp_protocol == OtlpProtocol::Grpc {
        tracing::warn!(
            "gRPC OTLP is not supported by r2e-observability; using OTLP/HTTP instead (normally port 4318)"
        );
    }

    OtelGuard { provider }
}

/// The tracer the contributed layer builds spans with — the provider of the
/// latest [`init_tracing`] call.
static CURRENT_TRACER: RwLock<Option<SdkTracer>> = RwLock::new(None);

/// Whether the OpenTelemetry layer and the id enricher are in the subscriber.
static CONTRIBUTED: Mutex<bool> = Mutex::new(false);

/// Warn about a hand-built subscriber at most once per process.
static NOT_WIRED_WARNED: AtomicBool = AtomicBool::new(false);

/// Contribute the OpenTelemetry layer and the [`OtelIds`] enricher, once per
/// process: two OpenTelemetry layers in one subscriber would both claim the
/// same span extension (and panic), and a second enricher would duplicate
/// the fields.
fn contribute_once() {
    let mut contributed = CONTRIBUTED.lock().unwrap_or_else(PoisonError::into_inner);
    if *contributed {
        return;
    }
    let layer = tracing_opentelemetry::layer().with_tracer(SharedTracer);
    match r2e_core::contribute_layer(layer.boxed()) {
        Ok(()) => {
            r2e_core::add_enricher(Box::new(OtelIds));
            *contributed = true;
        }
        Err(r2e_core::NotWired) => {
            if !NOT_WIRED_WARNED.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    "r2e-observability: this process runs a hand-built tracing subscriber that \
                     does not host R2E's late-layer slot, so no span is exported over OTLP. Add \
                     `r2e_core::late_layers()` first, directly on the `Registry` of that \
                     subscriber (or let R2E install it from the `tracing:` section)."
                );
            }
        }
    }
}

/// The tracer handed to the contributed `tracing-opentelemetry` layer: a
/// handle onto [`CURRENT_TRACER`], so the one layer of the process always
/// exports through the latest provider.
#[derive(Debug, Clone, Copy)]
struct SharedTracer;

impl Tracer for SharedTracer {
    type Span = opentelemetry_sdk::trace::Span;

    fn build_with_context(
        &self,
        builder: SpanBuilder,
        parent_cx: &opentelemetry::Context,
    ) -> Self::Span {
        let tracer = CURRENT_TRACER
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .expect("the OpenTelemetry layer is contributed after its tracer is set");
        tracer.build_with_context(builder, parent_cx)
    }
}

/// Adds `trace_id` and `span_id` to console lines emitted inside a span the
/// OpenTelemetry layer tracks. Lines outside a span, or inside an untraced
/// one, are left untouched.
///
/// Registered by the `Observability` plugin; public (hidden) so tests can
/// exercise it on a scoped subscriber.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default)]
pub struct OtelIds;

impl FieldEnricher for OtelIds {
    fn enrich(&self, cx: &EnrichContext<'_>, out: &mut dyn FnMut(&'static str, &dyn fmt::Display)) {
        if !cx.has_parent_span {
            return;
        }
        // The tracing-opentelemetry layer activates the span's OpenTelemetry
        // context on enter. Reading OTel's context directly avoids a
        // re-entrant tracing-dispatch lookup while the line is formatted.
        let context = opentelemetry::Context::current();
        let otel_span = context.span();
        let span_context = otel_span.span_context();
        if !span_context.is_valid() {
            return;
        }
        out("trace_id", &span_context.trace_id());
        out("span_id", &span_context.span_id());
    }
}

/// Normalize an OTLP/HTTP traces endpoint.
///
/// HTTP(S) endpoints without an explicit path receive the standard
/// `/v1/traces` suffix. Explicit paths and non-HTTP schemes are preserved.
pub fn normalized_otlp_traces_endpoint(endpoint: &str) -> String {
    let Ok(mut url) = url::Url::parse(endpoint) else {
        return endpoint.to_string();
    };
    if matches!(url.scheme(), "http" | "https") && matches!(url.path(), "" | "/") {
        url.set_path("/v1/traces");
        return url.to_string();
    }
    endpoint.to_string()
}

/// Event formatter that adds the active OpenTelemetry trace and span IDs.
///
/// Events outside an OpenTelemetry span are delegated unchanged, so the
/// formatter is also safe in tracing-only mode.
///
/// **Deprecated**: the `Observability` plugin no longer builds a subscriber;
/// it registers a field enricher on R2E's that adds the same fields.
#[deprecated(
    note = "replaced by the `trace_id`/`span_id` field enricher the `Observability` plugin registers on the framework subscriber; see docs/migration/observability-tracing.md"
)]
#[derive(Debug, Clone)]
pub struct TraceIdFormat<F> {
    inner: F,
    json: bool,
}

#[allow(deprecated)]
impl<F> TraceIdFormat<F> {
    /// Wrap a text event format.
    #[deprecated(
        note = "replaced by the `trace_id`/`span_id` field enricher the `Observability` plugin registers on the framework subscriber; see docs/migration/observability-tracing.md"
    )]
    pub fn text(inner: F) -> Self {
        Self { inner, json: false }
    }

    /// Wrap a JSON event format.
    #[deprecated(
        note = "replaced by the `trace_id`/`span_id` field enricher the `Observability` plugin registers on the framework subscriber; see docs/migration/observability-tracing.md"
    )]
    pub fn json(inner: F) -> Self {
        Self { inner, json: true }
    }
}

#[allow(deprecated)]
impl<S, N, F> FormatEvent<S, N> for TraceIdFormat<F>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
    F: FormatEvent<S, N>,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let mut rendered = String::new();
        self.inner
            .format_event(ctx, Writer::new(&mut rendered), event)?;

        let Some((trace_id, span_id)) = current_otel_ids(ctx) else {
            return writer.write_str(&rendered);
        };

        if self.json {
            write_json_ids(&mut writer, &rendered, trace_id, span_id)
        } else {
            write_text_ids(&mut writer, &rendered, trace_id, span_id)
        }
    }
}

fn current_otel_ids<S, N>(ctx: &FmtContext<'_, S, N>) -> Option<(String, String)>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    ctx.parent_span()?;
    // The tracing-opentelemetry layer activates the span's OpenTelemetry
    // context on enter. Reading OTel's context directly avoids a re-entrant
    // tracing-dispatch lookup while `FormatEvent` is running.
    let context = opentelemetry::Context::current();
    let otel_span = context.span();
    let span_context = otel_span.span_context();
    if !span_context.is_valid() {
        return None;
    }
    Some((
        span_context.trace_id().to_string(),
        span_context.span_id().to_string(),
    ))
}

fn write_text_ids(
    writer: &mut Writer<'_>,
    rendered: &str,
    trace_id: String,
    span_id: String,
) -> fmt::Result {
    let newline = rendered.find('\n').unwrap_or(rendered.len());
    writer.write_str(&rendered[..newline])?;
    write!(writer, " trace_id={trace_id} span_id={span_id}")?;
    writer.write_str(&rendered[newline..])
}

fn write_json_ids(
    writer: &mut Writer<'_>,
    rendered: &str,
    trace_id: String,
    span_id: String,
) -> fmt::Result {
    let Some(end) = rendered.rfind('}') else {
        return write_text_ids(writer, rendered, trace_id, span_id);
    };
    writer.write_str(&rendered[..end])?;
    write!(
        writer,
        ",\"trace_id\":\"{trace_id}\",\"span_id\":\"{span_id}\""
    )?;
    writer.write_str(&rendered[end..])
}

/// Guard that ensures traces are flushed when the application shuts down.
///
/// Holds the `SdkTracerProvider` and calls `shutdown()` on drop.
pub struct OtelGuard {
    provider: SdkTracerProvider,
}

impl Drop for OtelGuard {
    fn drop(&mut self) {
        if let Err(e) = self.provider.shutdown() {
            eprintln!("Failed to shut down OpenTelemetry tracer: {e}");
        }
    }
}
