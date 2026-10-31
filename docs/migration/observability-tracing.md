# Migration: `Observability` contributes to R2E's subscriber

Superseded before release by
[`tracing-single-path.md`](tracing-single-path.md): the `Observability` plugin
contributes its OpenTelemetry layer and `OtelIds` with
`PluginSetupContext::tracing_layer`, and `build_state()` installs them with
R2E's one subscriber. The `observability.tracing.*` → `tracing.*` move and the
deleted `ObservabilityConfig::with_log_format` / `with_tracing_config` /
`TraceIdFormat` are covered there.
