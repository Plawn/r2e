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
  - `rt::Instant` + `rt::sleep_until(deadline)` — the deadline form of
    `rt::sleep`, on the runtime's own monotonic clock; what a timer wheel driven
    by absolute fire times needs (the scheduler's min-heap driver).
  - `rt::yield_now()` and `rt::in_runtime()` — the latter is the non-panicking
    probe behind `current_handle`, for synchronous paths that may run outside a
    runtime (a `Drop` impl detaching cleanup work).
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

- **BREAKING (`r2e-events`, `r2e-scheduler`)**: the same flip reaches the
  event-bus and scheduler surfaces, which now speak `r2e::rt::CancelToken`:
  `BackendState::{poller_cancels, register_poller_cancel}` and
  `reconnect_loop(…, cancel: &CancelToken, …)` in `r2e-events`;
  `SchedulerHandle::{new, channel, token}`, `jobs_driver`, `start_jobs` and the
  `CancelToken` **bean** the `Scheduler` plugin provides (an app injecting the
  scheduler token writes `#[inject] cancel: CancelToken` now) in
  `r2e-scheduler`. `From` converts both ways with
  `tokio_util::sync::CancellationToken`, so a call site that needs the raw token
  adds `.into()`.

- **BREAKING (`r2e-core`)**: `ServiceComponent::start` now takes
  `r2e::rt::CancelToken` instead of `tokio_util::sync::CancellationToken`.
  Hand-written background services update their signature (`async fn start(self,
  shutdown: CancelToken)`); `#[derive(BackgroundService)]` users update the
  `run` method it delegates to (`async fn run(&self, shutdown: CancelToken)`).
  With that flip `r2e-tenant`, `r2e-data-sqlx` and `r2e-data-diesel` dropped
  their last `tokio-util` dependency.

- **`r2e-core` no longer depends on `tokio` / `tokio-util` / `tokio-stream` at
  all** (dev-dependencies aside): every internal call site — the builder and
  prepared-server paths, sharded serving, lazy-bean resolution, live-config
  watching, health, SSE/WS, dev-reload — goes through `r2e_core::rt`. Sharded
  serving in particular is now expressible on the facade thanks to two
  additions: `rt::RuntimeHandle` (a wrapper over `tokio::runtime::Handle`, now
  the type of `rt::current_handle`, `rt::control_plane_handle`,
  `rt::set_control_plane` and `Runtime::handle`) and `rt::TcpListener`
  (re-exported, since axum's `serve` takes the concrete type). Also new:
  `rt::block_in_place` and `CancelToken::cancelled_owned`.

- **`#[r2e::main]` / `#[r2e::test]` / `#[r2e::test_suite]` and
  `#[derive(BackgroundService)]` now emit facade paths** — the runtime is built
  through `<crate root>::rt::RuntimeBuilder` and the service token is
  `<crate root>::rt::CancelToken`, resolved through the same `r2e` /
  `r2e_core` root every other emitted path uses. **A generated project no
  longer needs `tokio` in its `Cargo.toml`** (`r2e new` stopped emitting it).
  `start_paused = true` needs the paused clock, now behind a forwarded feature:
  `r2e/test-util` → `r2e-core/test-util` → `r2e-rt/test-util`, which `r2e-test`
  turns on so it is present in any crate's dev graph and absent from release
  builds.

- **`clippy.toml`** grew a `disallowed-types` list —
  `tokio_util::sync::CancellationToken`, `tokio::task::JoinHandle`,
  `tokio::runtime::Handle` — next to the existing `disallowed-methods` deny on
  raw spawns. Runtime-neutral primitives (`tokio::sync::*`, `Instant`,
  `JoinSet`, …) stay allowed: they are re-exported by identity. The only
  exemptions are the `#[expect]`-marked wrapper definitions in `r2e-rt`.

- `r2e-events` (+ the `iggy` / `kafka` / `pulsar` / `rabbitmq` backends),
  `r2e-scheduler`, `r2e-executor` and `r2e-tenant` now go through the `rt`
  facade for spawning, timers, sync primitives and `select!`, and **dropped
  their direct `tokio` / `tokio-util` / `tokio-stream` dependencies**. No
  behaviour change; the four distributed backends needed no client-API escape
  hatch.

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
