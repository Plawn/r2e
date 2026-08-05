use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use r2e_observability::{inject_current_context, traced_reqwest_client};
use tracing_subscriber::layer::SubscriberExt;

#[test]
fn injects_the_current_span_context() {
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    let provider = SdkTracerProvider::builder().build();
    let tracer = provider.tracer("client-test");
    let subscriber = tracing_subscriber::Registry::default()
        .with(tracing_opentelemetry::layer().with_tracer(tracer));
    let subscriber_guard = tracing::subscriber::set_default(subscriber);

    let span = tracing::info_span!("outgoing call");
    let span_guard = span.enter();
    let mut headers = reqwest::header::HeaderMap::new();
    inject_current_context(&mut headers);

    let traceparent = headers["traceparent"].to_str().unwrap();
    let parts: Vec<_> = traceparent.split('-').collect();
    assert_eq!(parts.len(), 4);
    assert_eq!(parts[1].len(), 32);
    assert_eq!(parts[2].len(), 16);

    drop(span_guard);
    drop(subscriber_guard);
    provider.shutdown().unwrap();
}

#[test]
fn injection_is_a_noop_without_an_otel_span() {
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    let mut headers = reqwest::header::HeaderMap::new();
    inject_current_context(&mut headers);
    assert!(!headers.contains_key("traceparent"));

    let _client = traced_reqwest_client(reqwest::Client::builder().build().unwrap());
}
