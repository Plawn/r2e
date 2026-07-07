use r2e_core::{AppBuilder, BeanContext, BeanState, TCons, TNil};
use r2e_prometheus::prometheus::IntCounter;
use r2e_prometheus::{Prometheus, PrometheusRegistry};

// ── Minimal test state ─────────────────────────────────────────────────────

#[derive(Clone)]
struct TestState {
    #[allow(dead_code)]
    registry: PrometheusRegistry,
}

impl BeanState for TestState {
    type Requires = TCons<PrometheusRegistry, TNil>;
    fn from_context(ctx: &BeanContext) -> Self {
        Self {
            registry: ctx.get::<PrometheusRegistry>(),
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[r2e_core::test]
async fn plugin_provides_prometheus_registry() {
    let _app = AppBuilder::new()
        .plugin(Prometheus::new("/metrics"))
        .build_state::<TestState, _>()
        .await;

    // If we get here, the plugin successfully provided PrometheusRegistry
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
        .build_state::<TestState, _>()
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
        .build_state::<TestState, _>()
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

    // Also verify the bean was injected (access via bean_context is not directly
    // available, but if build_state succeeded, the bean was resolved)
    drop(app);
}
