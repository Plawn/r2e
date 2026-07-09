use r2e_core::type_list::BeanAccess;
use r2e_core::AppBuilder;
use r2e_prometheus::prometheus::IntCounter;
use r2e_prometheus::{Prometheus, PrometheusRegistry};

// ── Tests ──────────────────────────────────────────────────────────────────

#[r2e_core::test]
async fn plugin_provides_prometheus_registry() {
    let app = AppBuilder::new()
        .plugin(Prometheus::new("/metrics"))
        .build_state()
        .await;

    // The plugin registers `PrometheusRegistry` in the provision list, so it
    // must be present in the resolved HList state by type.
    let _registry: PrometheusRegistry = app.state().get::<PrometheusRegistry>();
}

#[r2e_core::test]
async fn builder_register_includes_custom_collectors() {
    let counter = IntCounter::new("plugin_test_counter", "test").unwrap();
    let counter_clone = counter.clone();

    let _app = AppBuilder::new()
        .plugin(
            Prometheus::builder()
                .endpoint("/metrics")
                .namespace("test")
                .register(Box::new(counter_clone))
                .build(),
        )
        .build_state()
        .await;

    // Increment the counter and verify it shows up in encoded output
    counter.inc();
    let output = r2e_prometheus::encode_metrics();
    assert!(
        output.contains("plugin_test_counter"),
        "custom counter registered via builder should appear in metrics output"
    );
}

#[r2e_core::test]
async fn registry_bean_can_register_at_runtime() {
    let app = AppBuilder::new()
        .plugin(Prometheus::new("/metrics"))
        .build_state()
        .await;

    // Access the registry through the global function
    let reg = r2e_prometheus::registry();
    let gauge =
        r2e_prometheus::prometheus::IntGauge::new("runtime_gauge", "registered at runtime")
            .unwrap();
    gauge.set(99);

    reg.register(Box::new(gauge))
        .expect("runtime registration should succeed");

    let output = r2e_prometheus::encode_metrics();
    assert!(
        output.contains("runtime_gauge"),
        "gauge registered at runtime should appear in output"
    );

    // Also verify the bean is present in the resolved HList state by type.
    let _registry: PrometheusRegistry = app.state().get::<PrometheusRegistry>();
}
