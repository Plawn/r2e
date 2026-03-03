use r2e_prometheus::prometheus::IntCounter;
use r2e_prometheus::{encode_metrics, init_metrics, is_initialized, registry, MetricsConfig};

// NOTE: All tests share the same process-level OnceLock<Metrics>.
// The first call to init_metrics() wins; subsequent calls return the existing instance.

#[test]
fn is_initialized_returns_true_after_init() {
    let config = MetricsConfig::default();
    init_metrics(&config);
    assert!(is_initialized());
}

#[test]
fn registry_returns_valid_registry() {
    let config = MetricsConfig::default();
    init_metrics(&config);
    let reg = registry();
    // Should be able to gather metrics from it
    let families = reg.gather();
    assert!(
        !families.is_empty(),
        "built-in HTTP metrics should be registered"
    );
}

#[test]
fn custom_collector_appears_in_encode() {
    let config = MetricsConfig::default();
    init_metrics(&config);

    let counter = IntCounter::new("test_custom_counter", "A test counter").unwrap();
    counter.inc();

    registry()
        .register(Box::new(counter))
        .expect("should register custom collector");

    let output = encode_metrics();
    assert!(
        output.contains("test_custom_counter"),
        "custom counter should appear in metrics output"
    );
}
