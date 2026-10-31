use r2e_observability::tracing_setup::normalized_otlp_traces_endpoint;

#[test]
fn normalizes_only_pathless_http_endpoints() {
    assert_eq!(
        normalized_otlp_traces_endpoint("http://localhost:4318"),
        "http://localhost:4318/v1/traces"
    );
    assert_eq!(
        normalized_otlp_traces_endpoint("https://collector.example/"),
        "https://collector.example/v1/traces"
    );
    assert_eq!(
        normalized_otlp_traces_endpoint("http://collector:4318/custom/traces"),
        "http://collector:4318/custom/traces"
    );
    assert_eq!(
        normalized_otlp_traces_endpoint("unix:///tmp/otel.sock"),
        "unix:///tmp/otel.sock"
    );
}
