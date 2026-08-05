use std::fmt;

use opentelemetry::trace::{TraceContextExt, TracerProvider};
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use opentelemetry_sdk::Resource;
use r2e_core::LogFormat;
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{layer::SubscriberExt, EnvFilter, Registry};

use crate::config::{ObservabilityConfig, OtlpProtocol};

/// Initialize the full tracing stack: console logs + OpenTelemetry export.
///
/// This replaces `r2e_core::init_tracing()` when observability is enabled.
/// Returns a guard that flushes traces on drop.
pub fn init_tracing(config: &ObservabilityConfig) -> OtelGuard {
    let grpc_requested = config.otlp_protocol == OtlpProtocol::Grpc;
    // Build resource attributes
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

    // Build the sampler
    let sampler = if config.sampling_ratio >= 1.0 {
        Sampler::AlwaysOn
    } else if config.sampling_ratio <= 0.0 {
        Sampler::AlwaysOff
    } else {
        Sampler::TraceIdRatioBased(config.sampling_ratio)
    };

    // Build the tracer provider
    let provider_builder = SdkTracerProvider::builder()
        .with_sampler(sampler)
        .with_resource(resource);

    // Add OTLP exporter if the feature is enabled
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

    let provider = provider_builder.build();
    let tracer = provider.tracer("r2e");

    // Build the tracing-subscriber stack using TracingConfig values.
    let tc = &config.tracing;
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&tc.filter));

    let span_events = tc.effective_span_events();
    let target = tc.target.unwrap_or(true);
    let thread_ids = tc.thread_ids.unwrap_or(false);
    let thread_names = tc.thread_names.unwrap_or(false);
    let file = tc.file.unwrap_or(false);
    let line_number = tc.line_number.unwrap_or(false);
    let level = tc.level.unwrap_or(true);
    let ansi = tc.ansi.unwrap_or(true);

    match tc.effective_format() {
        LogFormat::Json => {
            let event_format = tracing_subscriber::fmt::format()
                .json()
                .with_target(target)
                .with_thread_ids(thread_ids)
                .with_thread_names(thread_names)
                .with_file(file)
                .with_line_number(line_number)
                .with_level(level)
                .with_ansi(ansi);
            let mut fmt_layer = tracing_subscriber::fmt::layer()
                .fmt_fields(tracing_subscriber::fmt::format::JsonFields::new())
                .event_format(TraceIdFormat::json(event_format));
            fmt_layer.set_span_events(span_events);

            let subscriber = Registry::default()
                .with(env_filter)
                .with(fmt_layer)
                .with(tracing_opentelemetry::layer().with_tracer(tracer));

            if tracing::subscriber::set_global_default(subscriber).is_err() {
                tracing::warn!("A global tracing subscriber was already set (e.g. by dioxus-devtools). Observability tracing layer skipped.");
            }
        }
        LogFormat::Pretty => {
            let event_format = tracing_subscriber::fmt::format()
                .with_target(target)
                .with_thread_ids(thread_ids)
                .with_thread_names(thread_names)
                .with_file(file)
                .with_line_number(line_number)
                .with_level(level)
                .with_ansi(ansi);
            let mut fmt_layer =
                tracing_subscriber::fmt::layer().event_format(TraceIdFormat::text(event_format));
            fmt_layer.set_span_events(span_events);

            let subscriber = Registry::default()
                .with(env_filter)
                .with(fmt_layer)
                .with(tracing_opentelemetry::layer().with_tracer(tracer));

            if tracing::subscriber::set_global_default(subscriber).is_err() {
                tracing::warn!("A global tracing subscriber was already set (e.g. by dioxus-devtools). Observability tracing layer skipped.");
            }
        }
    }

    if grpc_requested {
        tracing::warn!(
            "gRPC OTLP is not supported by r2e-observability; using OTLP/HTTP instead (normally port 4318)"
        );
    }

    OtelGuard { provider }
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
#[derive(Debug, Clone)]
pub struct TraceIdFormat<F> {
    inner: F,
    json: bool,
}

impl<F> TraceIdFormat<F> {
    pub fn text(inner: F) -> Self {
        Self { inner, json: false }
    }

    pub fn json(inner: F) -> Self {
        Self { inner, json: true }
    }
}

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
