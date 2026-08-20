# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Changed

- **r2e-observability**: `traced_reqwest_client` / `TraceContextMiddleware`
  now open an OpenTelemetry **client** span per outgoing request
  (`otel.kind = "client"`, name `HTTP {method}`, HTTP-client semantic
  conventions: `http.request.method`, `server.address`, `server.port`,
  `url.full`, `http.response.status_code`, `otel.status_code` /
  `error.message`) and propagate **that span's** context instead of the
  caller's. Tracing backends that derive a service graph from CLIENT→SERVER
  pairs (Tempo metrics-generator, Jaeger, Grafana) now show `caller → callee`
  edges and client-side latency for R2E services calling each other.
  Implemented on `reqwest-tracing` pinned to the workspace
  `opentelemetry 0.32` / `tracing-opentelemetry 0.33`. New re-exports:
  `R2eSpanBackend`, `OtelName`, `OtelPathNames`, `DisableOtelPropagation`.
  `inject_current_context` is unchanged (headers only, no client span).
  Follow-up of the outgoing-propagation work (#764, #765, #766); task #927.
