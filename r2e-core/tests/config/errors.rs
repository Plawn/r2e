//! `ConfigError` variants and their rendering.

use r2e_core::config::ConfigError;

// =========================================================================
// ConfigError::Validation
// =========================================================================

#[test]
fn test_config_validation_error_display() {
    use r2e_core::config::ConfigValidationDetail;
    let err = ConfigError::Validation(vec![ConfigValidationDetail {
        key: "app.port".to_string(),
        message: "must be between 1 and 65535".to_string(),
    }]);
    let msg = err.to_string();
    assert!(msg.contains("app.port"));
    assert!(msg.contains("must be between 1 and 65535"));
}

// =========================================================================
// ConfigError::Deserialize display
// =========================================================================

#[test]
fn test_config_deserialize_error_display() {
    let err = ConfigError::Deserialize {
        key: "app.mode".to_string(),
        message: "unknown variant `bad`".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("app.mode"));
    assert!(msg.contains("unknown variant `bad`"));
}
