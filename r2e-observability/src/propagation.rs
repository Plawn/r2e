use opentelemetry_sdk::propagation::TraceContextPropagator;

use crate::config::{ObservabilityConfig, PropagationFormat};

/// Install the global text-map propagator based on config.
///
/// This must be called before any trace context extraction/injection occurs.
pub fn install_propagator(config: &ObservabilityConfig) {
    // All formats currently map to W3C TraceContext propagation.
    // For full B3/Jaeger propagation, additional crates would be needed.
    let _format = &config.propagation_format;
    match _format {
        PropagationFormat::W3c | PropagationFormat::B3 | PropagationFormat::Jaeger => {
            opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
        }
    }
}
