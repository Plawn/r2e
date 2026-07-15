use r2e_core::http::{Body, Request, StatusCode};
use r2e_core::type_list::BeanAccess;
use r2e_core::{AppBuilder, R2eConfig};
use r2e_prometheus::prometheus::IntCounter;
use r2e_prometheus::{resolve_config, Prometheus, PrometheusConfig, PrometheusRegistry};
use tower::ServiceExt;

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

// ── Typed config: precedence + loading + validation (Phase 4) ────────────────

#[test]
fn resolve_config_builder_setting_wins_over_file() {
    // Every knob is set by BOTH the builder and file config; the builder wins.
    let (endpoint, cfg) = resolve_config(
        Some("/builder".into()),
        Some("builder_ns".into()),
        Some(vec![1.0, 2.0]),
        Some(vec!["/builder-skip".into()]),
        Some(PrometheusConfig {
            endpoint: Some("/file".into()),
            namespace: Some("file_ns".into()),
            buckets: Some(vec![9.0]),
            exclude_paths: Some(vec!["/file-skip".into()]),
        }),
    );
    assert_eq!(endpoint, "/builder");
    assert_eq!(cfg.namespace.as_deref(), Some("builder_ns"));
    assert_eq!(cfg.buckets, vec![1.0, 2.0]);
    assert_eq!(cfg.exclude_paths, vec!["/builder-skip".to_string()]);
}

#[test]
fn resolve_config_file_wins_over_default() {
    // Builder set nothing; file config supplies every knob.
    let (endpoint, cfg) = resolve_config(
        None,
        None,
        None,
        None,
        Some(PrometheusConfig {
            endpoint: Some("/file".into()),
            namespace: Some("file_ns".into()),
            buckets: Some(vec![0.5, 1.5]),
            exclude_paths: Some(vec!["/file-skip".into()]),
        }),
    );
    assert_eq!(endpoint, "/file");
    assert_eq!(cfg.namespace.as_deref(), Some("file_ns"));
    assert_eq!(cfg.buckets, vec![0.5, 1.5]);
    assert_eq!(cfg.exclude_paths, vec!["/file-skip".to_string()]);
}

#[test]
fn resolve_config_falls_back_to_defaults() {
    // Neither builder nor file set anything → built-in defaults.
    let (endpoint, cfg) = resolve_config(None, None, None, None, None);
    assert_eq!(endpoint, "/metrics");
    assert_eq!(cfg.namespace, None);
    assert!(cfg.exclude_paths.is_empty());
    assert!(!cfg.buckets.is_empty(), "default buckets are populated");
}

async fn status_of(router: r2e_core::http::Router, path: &str) -> StatusCode {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    router.oneshot(req).await.unwrap().status()
}

#[r2e_core::test]
async fn endpoint_is_driven_by_file_config() {
    // The builder does NOT set an endpoint, so the `prometheus.endpoint` file
    // value drives the metrics route. This proves file config reaches
    // `configure`.
    let config =
        R2eConfig::from_yaml_str("prometheus:\n  endpoint: /custom-metrics\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Prometheus::builder().build())
        .build_state()
        .await;

    let router = app.build();
    assert_eq!(
        status_of(router.clone(), "/custom-metrics").await,
        StatusCode::OK,
        "endpoint from file config is served"
    );
    assert_eq!(
        status_of(router, "/metrics").await,
        StatusCode::NOT_FOUND,
        "the default endpoint is not mounted when file config overrides it"
    );
}

#[r2e_core::test]
async fn builder_endpoint_wins_over_file_config() {
    // `Prometheus::new` sets the endpoint explicitly, so it beats file config.
    let config =
        R2eConfig::from_yaml_str("prometheus:\n  endpoint: /from-file\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Prometheus::new("/from-builder"))
        .build_state()
        .await;

    let router = app.build();
    assert_eq!(
        status_of(router.clone(), "/from-builder").await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(router, "/from-file").await,
        StatusCode::NOT_FOUND
    );
}

#[r2e_core::test]
#[should_panic(expected = "Invalid configuration for plugin")]
async fn malformed_config_section_panics_at_boot() {
    // `prometheus.buckets` is a scalar string where a list of floats is
    // required — the same class of error a malformed controller `#[config]`
    // produces. Boot must fail with a validation error naming the plugin.
    let config = R2eConfig::from_yaml_str("prometheus:\n  buckets: not-a-list\n").unwrap();
    let _app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Prometheus::builder().build())
        .build_state()
        .await;
}

#[r2e_core::test]
async fn disabled_via_config_skips_route_and_layer_but_keeps_registry() {
    // `prometheus.enabled: false` gates the plugin's post-state effects: the
    // `configure` step (which mounts the /metrics route and the tracking layer)
    // is skipped. The `PrometheusRegistry` bean is still provided, because the
    // type-level provision list is fixed at compile time.
    let config = R2eConfig::from_yaml_str("prometheus:\n  enabled: false\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Prometheus::new("/metrics"))
        .build_state()
        .await;

    // Registry bean survives the disable — and stays USABLE: its accessors
    // lazily default-initialize the global registry instead of panicking
    // ("beans remain in the graph" contract of the enabled gate). The global
    // is process-wide, so an enabled test may have initialized it first; the
    // assertion that matters here is "no panic + registrable".
    let registry: PrometheusRegistry = app.state().get::<PrometheusRegistry>();
    let counter = prometheus::Counter::new("disabled_plugin_probe", "probe").unwrap();
    registry
        .register(Box::new(counter))
        .expect("handle of a disabled plugin must accept collectors without panicking");

    // But the /metrics route was never mounted.
    let router = app.build();
    assert_eq!(
        status_of(router, "/metrics").await,
        StatusCode::NOT_FOUND,
        "disabled plugin mounts no /metrics route"
    );
}

#[r2e_core::test]
async fn enabled_true_via_config_mounts_route() {
    // Explicit `enabled: true` behaves like the default: the /metrics route is
    // mounted.
    let config = R2eConfig::from_yaml_str("prometheus:\n  enabled: true\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Prometheus::new("/metrics"))
        .build_state()
        .await;

    assert_eq!(status_of(app.build(), "/metrics").await, StatusCode::OK);
}
