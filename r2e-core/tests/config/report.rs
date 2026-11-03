//! The one configuration-error banner (#1214): every boot-time config failure
//! (controller / endpoint / service registration, module validation, MCP and
//! gRPC service registration, `server.drain-timeout`) renders through
//! `r2e_core::config::ConfigErrorReport`. These tests pin the exact bytes, so
//! a change to the banner is a deliberate, visible diff.

use r2e_core::beans::BeanError;
use r2e_core::config::{ConfigErrorReport, ConfigValidationError, MissingKeyError, R2eConfig};

fn report() -> ConfigValidationError {
    ConfigValidationError {
        errors: vec![MissingKeyError {
            source: "MyController".into(),
            key: "app.name".into(),
            expected_type: "String".into(),
            env_hint: Some("R2E_APP_NAME".into()),
            description: None,
        }],
    }
}

const BODY: &str = "Missing configuration keys:\n  - `MyController`: key 'app.name' (String) — set env var `R2E_APP_NAME`\n";

#[test]
fn controller_config_bean_error_banner() {
    let err = BeanError::ControllerConfig {
        controller: "my_app::MyController",
        source: report(),
    };
    assert_eq!(
        err.to_string(),
        format!(
            "\n=== CONFIGURATION ERRORS (controller: my_app::MyController) ===\n\n{BODY}\n============================\n"
        )
    );
}

#[test]
fn endpoint_config_bean_error_banner() {
    let err = BeanError::EndpointConfig {
        endpoint: "my_app::Greeter",
        source: report(),
    };
    assert_eq!(
        err.to_string(),
        format!(
            "\n=== CONFIGURATION ERRORS (endpoint: my_app::Greeter) ===\n\n{BODY}\n============================\n"
        )
    );
}

#[test]
fn drain_timeout_banner() {
    let mut config = R2eConfig::empty();
    config.set(
        r2e_core::runtime::drain::DRAIN_TIMEOUT_CONFIG_KEY,
        r2e_core::config::ConfigValue::String("soon".into()),
    );
    let panic = std::panic::catch_unwind(|| {
        r2e_core::runtime::drain::resolve_drain_timeout(None, Some(&config))
    })
    .expect_err("a malformed drain timeout fails the boot");
    let msg = panic
        .downcast_ref::<String>()
        .cloned()
        .expect("formatted panic message");
    let err = config
        .get::<std::time::Duration>("server.drain-timeout")
        .unwrap_err();
    assert_eq!(
        msg,
        format!(
            "\n=== CONFIGURATION ERRORS ===\n\nserver.drain-timeout: {err}\n\
             expected a duration (integer seconds, or \"30s\" / \"500ms\" / \"2m\")\n\
             ============================\n"
        )
    );
}

/// The registration panics (`register_controller`, `spawn_service`,
/// `register_mcp_service`, `register_grpc_service`) print the scoped report
/// with the type name as subject: same bytes as the literal each one used to
/// carry before the formatter existed.
#[test]
fn scoped_report_matches_the_former_registration_literals() {
    let err = report();
    for kind in ["controller", "service", "mcp service", "grpc service"] {
        let subject = "my_app::Thing";
        let former = format!(
            "\n=== CONFIGURATION ERRORS ({kind}: {}) ===\n\n{}\n============================\n",
            subject, err
        );
        assert_eq!(
            ConfigErrorReport::scoped(kind, &subject, &err).to_string(),
            former,
            "kind {kind}"
        );
    }
}

#[test]
fn unscoped_report_has_no_scope_suffix() {
    assert_eq!(
        ConfigErrorReport::unscoped(&"server.x: bad").to_string(),
        "\n=== CONFIGURATION ERRORS ===\n\nserver.x: bad\n============================\n"
    );
}
