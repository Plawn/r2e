use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::{SdkTracerProvider, Sampler};
use opentelemetry_sdk::Resource;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Registry};

use crate::config::{LogFormat, ObservabilityConfig};

/// Initialize the full tracing stack: console logs + OpenTelemetry export.
///
/// This replaces `r2e_core::init_tracing()` when observability is enabled.
/// Returns a guard that flushes traces on drop.
pub fn init_tracing(config: &ObservabilityConfig) -> OtelGuard {
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
    let mut provider_builder = SdkTracerProvider::builder()
        .with_sampler(sampler)
        .with_resource(resource);

    // Add OTLP exporter if the feature is enabled
    #[cfg(feature = "otlp")]
    {
        use opentelemetry_otlp::WithExportConfig;
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(&config.otlp_endpoint)
            .build()
            .expect("Failed to build OTLP span exporter");
        provider_builder = provider_builder.with_batch_exporter(exporter);
    }

    let provider = provider_builder.build();
    let tracer = provider.tracer("r2e");

    // Build the tracing-subscriber stack.
    // The OTel layer must be created inside each match arm because its type
    // depends on the subscriber type (which differs for JSON vs Pretty fmt).
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,tower_http=debug"));

    match config.log_format {
        LogFormat::Json => {
            let fmt_layer = tracing_subscriber::fmt::layer()
                .json()
                .with_target(true)
                .with_thread_ids(false)
                .with_file(false)
                .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE);

            Registry::default()
                .with(env_filter)
                .with(fmt_layer)
                .with(tracing_opentelemetry::layer().with_tracer(tracer))
                .init();
        }
        LogFormat::Pretty => {
            let fmt_layer = tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_thread_ids(false)
                .with_file(false)
                .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE);

            Registry::default()
                .with(env_filter)
                .with(fmt_layer)
                .with(tracing_opentelemetry::layer().with_tracer(tracer))
                .init();
        }
    }

    OtelGuard { provider }
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
