//! `Observability` contributes to R2E's one subscriber instead of building
//! its own.
//!
//! Own binary: the plugin's `setup` installs the process-global subscriber
//! (through `ensure_tracing_installed`, from the — absent — `tracing:`
//! section) and contributes the OpenTelemetry layer to its late-layer slot,
//! once per process. Every test shares that one boot ([`boot`]) and tells its
//! spans apart by name. Spans go to an in-memory exporter through the hidden
//! `with_span_exporter` seam.

use std::collections::HashMap;
use std::sync::OnceLock;

use opentelemetry::propagation::TextMapPropagator;
use opentelemetry::trace::TraceContextExt;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::InMemorySpanExporter;
use opentelemetry_sdk::trace::SpanData;
use r2e_core::AppBuilder;
use r2e_observability::{Observability, ObservabilityConfig};
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Boot the plugin once for the whole binary and return its exporter.
///
/// The builder holding the tracer-provider guard is leaked on purpose:
/// dropping it would shut the provider down (and empty the exporter).
fn boot() -> &'static InMemorySpanExporter {
    static EXPORTER: OnceLock<InMemorySpanExporter> = OnceLock::new();
    EXPORTER.get_or_init(|| {
        let exporter = InMemorySpanExporter::default();
        let builder = AppBuilder::new().plugin(
            Observability::new(ObservabilityConfig::new("otel-contrib"))
                .with_span_exporter(exporter.clone()),
        );
        std::mem::forget(builder);
        exporter
    })
}

fn exported(exporter: &InMemorySpanExporter, name: &str) -> SpanData {
    exporter
        .get_finished_spans()
        .unwrap()
        .into_iter()
        .find(|span| span.name == name)
        .unwrap_or_else(|| panic!("span `{name}` was not exported"))
}

#[test]
fn setup_installs_the_framework_subscriber() {
    boot();
    assert!(
        r2e_core::r2e_tracing::installed().is_some(),
        "the plugin must install R2E's subscriber when nothing did"
    );
}

#[test]
fn spans_and_their_events_are_exported_through_the_late_layer_slot() {
    let exporter = boot();
    {
        let span = tracing::info_span!("contrib_export");
        let _entered = span.enter();
        tracing::info!("inside the exported span");
    }

    let span = exported(exporter, "contrib_export");
    assert!(
        span.events
            .iter()
            .any(|event| event.name == "inside the exported span"),
        "events: {:?}",
        span.events
    );
}

#[test]
fn span_context_is_reachable_through_the_slot() {
    boot();
    let span = tracing::info_span!("contrib_context");
    let _entered = span.enter();

    // `OpenTelemetrySpanExt::context` downcasts the dispatcher to
    // tracing-opentelemetry's `WithContext`: it only works when the slot
    // forwards `downcast_raw` to the contributed layer.
    let context = tracing::Span::current().context();
    assert!(context.span().span_context().is_valid());
}

#[test]
fn an_incoming_traceparent_becomes_the_parent() {
    let exporter = boot();
    let trace_id = "4bf92f3577b34da6a3ce929d0e0e4736";
    let parent_span_id = "00f067aa0ba902b7";
    let headers = HashMap::from([(
        "traceparent".to_string(),
        format!("00-{trace_id}-{parent_span_id}-01"),
    )]);
    let remote = TraceContextPropagator::new().extract(&headers);

    {
        let span = tracing::info_span!("contrib_traceparent");
        span.set_parent(remote).unwrap();
        let _entered = span.enter();
        tracing::info!("handled");
    }

    let span = exported(exporter, "contrib_traceparent");
    assert_eq!(span.span_context.trace_id().to_string(), trace_id);
    assert_eq!(span.parent_span_id.to_string(), parent_span_id);
}

#[test]
fn noise_rules_gate_the_exported_events_too() {
    let exporter = boot();
    r2e_core::suppress_error("otel_contrib::noise", "benign upstream disconnect");
    {
        let span = tracing::info_span!("contrib_noise");
        let _entered = span.enter();
        tracing::error!(target: "otel_contrib::noise", "benign upstream disconnect");
        tracing::error!(target: "otel_contrib::noise", "real failure");
    }

    let span = exported(exporter, "contrib_noise");
    let names: Vec<&str> = span.events.iter().map(|e| e.name.as_ref()).collect();
    assert!(names.contains(&"real failure"), "events: {names:?}");
    assert!(
        !names.contains(&"benign upstream disconnect"),
        "a suppressed event reached the OpenTelemetry layer: {names:?}"
    );
}
