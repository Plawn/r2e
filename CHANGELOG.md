# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- **New crate `r2e-rt`** — the async-runtime facade, sitting at the **bottom**
  of the workspace dependency graph (below `r2e-http`). It is now the single
  workspace member allowed to name `tokio` / `tokio-util` / `tokio-stream`
  directly, so swapping the runtime — or moving further towards thread-per-core
  sharded runtimes — is a change in one crate instead of a hunt across dozens of
  call sites. Two enforcement scripts freeze the boundary
  (`scripts/check-dep-boundary.sh`, `scripts/check-source-boundary.sh`).
  `r2e-core/src/rt.rs` moved into it wholesale and `r2e_core::rt` is now a
  re-export, so **`r2e::rt::…` / `r2e_core::rt::…` keep resolving to exactly
  what they always did** (`spawn`, `spawn_ctl`, `spawn_blocking`, `JobHandle`,
  `sleep`, `timeout`, `interval`, `bind_tcp`, `shutdown_signal`, …).
  New in the facade, on top of the moved surface:
  - `rt::CancelToken` / `rt::CancelDropGuard` — wrappers over
    `tokio_util::sync::CancellationToken` / `DropGuard`, so an app can consume
    R2E's shutdown API without adding `tokio-util` to its own `Cargo.toml`.
    `From` conversions both ways keep the not-yet-migrated crates working.
  - `rt::sync` — re-exports of `mpsc`, `oneshot`, `broadcast`, `watch`,
    `Mutex`, `RwLock`, `Notify`, `Semaphore`, `OnceCell`.
  - `rt::{select!, pin!, join!}`, `rt::JoinSet`, `rt::stream`,
    `rt::{RuntimeBuilder, Runtime, block_on}`.
  - A non-default `test-util` feature (`tokio/test-util`), off by default
    because paused clocks must not reach the whole workspace through feature
    unification.

### Changed

- **BREAKING (`r2e-core`)**: the shutdown-token surface now hands out
  `r2e::rt::CancelToken` instead of `tokio_util::sync::CancellationToken` —
  `ServeContext::shutdown_token()`, `ConfigWatchContext::{new, shutdown_token}`
  and `LiveConfigReceiver::drive`. Call sites that only `select!` on the token
  or pass it along are unaffected; a site that needs the raw tokio-util token
  (tonic's `cancelled_owned()`, say) converts with `.into()` / `.into_inner()`.

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
