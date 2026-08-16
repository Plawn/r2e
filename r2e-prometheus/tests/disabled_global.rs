//! A disabled Prometheus plugin must not touch the process-global metrics
//! registry.
//!
//! This lives in its own test target on purpose: `METRICS` is a
//! `OnceLock` — any other test in the same binary that boots an enabled plugin
//! (or merely reads `registry()`) initializes it, and the assertion below could
//! then only ever pass by accident. One test, one process.

use r2e_core::config::R2eConfig;
use r2e_core::type_list::BeanAccess;
use r2e_core::AppBuilder;
use r2e_prometheus::{Prometheus, PrometheusRegistry};

#[r2e_core::test]
async fn a_disabled_plugin_leaves_the_global_recorder_untouched() {
    assert!(
        !r2e_prometheus::is_initialized(),
        "pre-condition: nothing in this process has initialized the global \
         metrics yet"
    );

    let config = R2eConfig::from_yaml_str("prometheus:\n  enabled: false\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Prometheus::new("/metrics"))
        .build_state()
        .await;

    // The bean exists (the provision list is fixed at compile time) …
    let _registry: PrometheusRegistry = app.state().get::<PrometheusRegistry>();
    // … but `build` returned before `init_metrics`: no collectors registered,
    // no default recorder installed behind the app's back. Disabling a plugin
    // has to make it INERT, not merely unmounted — the effect gate alone would
    // not have caught this, since `init_metrics` runs inside `build`.
    assert!(
        !r2e_prometheus::is_initialized(),
        "a disabled plugin installed the global metrics registry"
    );
}
