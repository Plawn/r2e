use std::sync::{PoisonError, RwLock};

use opentelemetry::trace::{SpanBuilder, TraceContextExt, Tracer, TracerProvider};
use opentelemetry_sdk::trace::{
    Sampler, SdkTracer, SdkTracerProvider, SpanExporter, TracerProviderBuilder,
};
use opentelemetry_sdk::Resource;
use r2e_core::LogFields;
use tracing::span::Id;
use tracing::Subscriber;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

use crate::config::{ObservabilityConfig, OtlpProtocol};

/// Start OpenTelemetry export: build the tracer provider (resource, sampler,
/// OTLP/HTTP exporter) the [`otel_layer`] exports through.
///
/// It touches no subscriber: the `Observability` plugin contributes
/// [`otel_layer`] and [`OtelIds`] with `PluginSetupContext::tracing_layer`,
/// and `build_state()` installs them with R2E's one subscriber. A later call
/// (a second app boot in the same process, a dev-reload cycle) points the
/// already-installed layer at the new provider.
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

/// The `tracing-opentelemetry` layer, exporting through the provider of the
/// latest [`init_tracing`] call. Contribute it **before** [`OtelIds`], which
/// reads the OpenTelemetry context this layer activates on span enter.
pub fn otel_layer<S>() -> impl Layer<S> + Send + Sync + 'static
where
    S: Subscriber + for<'a> LookupSpan<'a> + Send + Sync,
{
    tracing_opentelemetry::layer().with_tracer(SharedTracer)
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

/// Writes `trace_id` and `span_id` as [`LogFields`] on every span the
/// OpenTelemetry layer tracks, so R2E's console prints them on each line
/// emitted inside it. Spans outside a valid OpenTelemetry context are left
/// untouched.
///
/// The ids are read on the first **enter**: that is when the
/// `tracing-opentelemetry` layer (contributed before this one) starts the
/// span and activates its context.
#[derive(Debug, Clone, Copy, Default)]
pub struct OtelIds;

impl<S> Layer<S> for OtelIds
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        if span
            .extensions()
            .get::<LogFields>()
            .is_some_and(|fields| fields.iter().any(|(k, _)| k == "trace_id"))
        {
            return;
        }
        let context = opentelemetry::Context::current();
        let otel_span = context.span();
        let span_context = otel_span.span_context();
        if !span_context.is_valid() {
            return;
        }
        let (trace_id, span_id) = (span_context.trace_id(), span_context.span_id());
        let mut extensions = span.extensions_mut();
        match extensions.get_mut::<LogFields>() {
            Some(fields) => {
                fields.insert("trace_id", trace_id);
                fields.insert("span_id", span_id);
            }
            None => {
                let mut fields = LogFields::new();
                fields.insert("trace_id", trace_id);
                fields.insert("span_id", span_id);
                extensions.insert(fields);
            }
        }
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
