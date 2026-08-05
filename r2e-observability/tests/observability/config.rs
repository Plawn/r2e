use std::collections::HashMap;
use std::sync::Mutex;

use r2e_observability::{Observability, ObservabilityConfig, OtlpProtocol};

static ENV_LOCK: Mutex<()> = Mutex::new(());

const OTEL_KEYS: &[&str] = &[
    "OTEL_EXPORTER_OTLP_ENDPOINT",
    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
    "OTEL_EXPORTER_OTLP_PROTOCOL",
    "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
    "OTEL_SERVICE_NAME",
    "OTEL_TRACES_SAMPLER",
    "OTEL_TRACES_SAMPLER_ARG",
    "OTEL_SDK_DISABLED",
];

struct EnvSnapshot(HashMap<&'static str, Option<String>>);

impl EnvSnapshot {
    fn clear() -> Self {
        let values = OTEL_KEYS
            .iter()
            .copied()
            .map(|key| (key, std::env::var(key).ok()))
            .collect();
        for key in OTEL_KEYS {
            std::env::remove_var(key);
        }
        Self(values)
    }
}

impl Drop for EnvSnapshot {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[test]
fn from_env_uses_standard_otel_variables() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _snapshot = EnvSnapshot::clear();
    std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318");
    std::env::set_var("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL", "http/protobuf");
    std::env::set_var("OTEL_SERVICE_NAME", "env-service");
    std::env::set_var("OTEL_TRACES_SAMPLER", "parentbased_traceidratio");
    std::env::set_var("OTEL_TRACES_SAMPLER_ARG", "0.25");

    let config = ObservabilityConfig::from_env("fallback-service");
    assert_eq!(config.service_name, "env-service");
    assert_eq!(config.otlp_endpoint, "http://collector:4318");
    assert_eq!(config.otlp_protocol, OtlpProtocol::Http);
    assert_eq!(config.sampling_ratio, 0.25);
    assert!(Observability::from_env("fallback-service").is_otlp_enabled());
}

#[test]
fn from_env_falls_back_to_standard_tracing_without_an_endpoint() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _snapshot = EnvSnapshot::clear();

    let observability = Observability::from_env("fallback-service");
    assert!(!observability.is_otlp_enabled());
    assert_eq!(
        ObservabilityConfig::from_env("fallback-service").service_name,
        "fallback-service"
    );
}
