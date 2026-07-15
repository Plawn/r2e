use r2e_core::builder::AppBuilder;
use r2e_core::config::R2eConfig;

fn prepare_with_yaml(yaml: &str) -> r2e_core::builder::PreparedApp<()> {
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    AppBuilder::new()
        .override_config(config).load_config::<()>()
        .with_state(())
        .prepare("0.0.0.0:0")
}

#[test]
fn tcp_nodelay_defaults_to_true_without_config() {
    let app = AppBuilder::new().with_state(()).prepare("0.0.0.0:0");
    assert!(app.tcp_nodelay());
}

#[test]
fn tcp_nodelay_defaults_to_true_when_key_absent() {
    let app = prepare_with_yaml("server:\n  port: 3000\n");
    assert!(app.tcp_nodelay());
}

#[test]
fn tcp_nodelay_true_when_explicitly_set() {
    let app = prepare_with_yaml("server:\n  tcp_nodelay: true\n");
    assert!(app.tcp_nodelay());
}

#[test]
fn tcp_nodelay_false_when_explicitly_disabled() {
    let app = prepare_with_yaml("server:\n  tcp_nodelay: false\n");
    assert!(!app.tcp_nodelay());
}
