# Changelog — r2e

All notable changes to `r2e` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the version is this
crate's own (pre-1.0: a breaking change bumps the minor, anything else the
patch).

Write entries under `[Unreleased]` in the PR that makes the change. A release
PR (`scripts/bump-version.sh`) turns that section into `[X.Y.Z] - date`, and
`.github/workflows/release.yml` publishes the crate, tags `r2e-vX.Y.Z` and
uses the section as the GitHub release notes.

Up to 0.6.0 the whole workspace was released in lockstep; that history lives
in the root [`CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

## [0.10.0] - 2026-10-31

### Breaking

- One tracing path: the subscriber is installed by `build_state()` from the
  `tracing:` section plus the layers contributed with `.tracing_layer(..)`
  (r2e-core). `app_main!(App, tracing = …)` and `launch!(App, tracing = …)`
  are compile errors naming the migration page; `launch!` captures the boot
  logs before `App::setup` (under `dev-reload`, once, outside the patch
  loop). The `r2e-macros` `tracing = …` argument removal and every `r2e-core`
  tracing removal reach apps through this facade. Migration:
  `docs/migration/tracing-single-path.md`.

### Changed

- Built against `r2e-core` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.9.0/r2e-core/CHANGELOG.md)).
- Built against `r2e-macros` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-macros/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-macros-v0.9.0/r2e-macros/CHANGELOG.md)).
- Built against `r2e-data-diesel` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-data-diesel/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-data-diesel-v0.9.0/r2e-data/backends/diesel/CHANGELOG.md)).
- Built against `r2e-data-sqlx` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-data-sqlx/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-data-sqlx-v0.9.0/r2e-data/backends/sqlx/CHANGELOG.md)).
- Built against `r2e-events` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-events/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-events-v0.9.0/r2e-events/CHANGELOG.md)).
- Built against `r2e-events-iggy` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-events-iggy/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-events-iggy-v0.9.0/r2e-events/backends/iggy/CHANGELOG.md)).
- Built against `r2e-events-kafka` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-events-kafka/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-events-kafka-v0.9.0/r2e-events/backends/kafka/CHANGELOG.md)).
- Built against `r2e-events-pulsar` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-events-pulsar/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-events-pulsar-v0.9.0/r2e-events/backends/pulsar/CHANGELOG.md)).
- Built against `r2e-events-rabbitmq` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-events-rabbitmq/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-events-rabbitmq-v0.9.0/r2e-events/backends/rabbitmq/CHANGELOG.md)).
- Built against `r2e-executor` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-executor/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-executor-v0.9.0/r2e-executor/CHANGELOG.md)).
- Built against `r2e-grpc` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-grpc/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-grpc-v0.9.0/r2e-grpc/CHANGELOG.md)).
- Built against `r2e-mcp` 0.10.0 — its breaking changes surface through this crate's re-exports (see [`r2e-mcp/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-mcp-v0.10.0/r2e-mcp/CHANGELOG.md)).
- Built against `r2e-observability` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-observability/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-observability-v0.9.0/r2e-observability/CHANGELOG.md)).
- Built against `r2e-oidc` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-oidc/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-oidc-v0.9.0/r2e-oidc/CHANGELOG.md)).
- Built against `r2e-openapi` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-openapi/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-openapi-v0.9.0/r2e-openapi/CHANGELOG.md)).
- Built against `r2e-openfga` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-openfga/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-openfga-v0.9.0/r2e-openfga/CHANGELOG.md)).
- Built against `r2e-prometheus` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-prometheus/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-prometheus-v0.9.0/r2e-prometheus/CHANGELOG.md)).
- Built against `r2e-rate-limit` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-rate-limit/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-rate-limit-v0.9.0/r2e-rate-limit/CHANGELOG.md)).
- Built against `r2e-scheduler` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-scheduler/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-scheduler-v0.9.0/r2e-scheduler/CHANGELOG.md)).
- Built against `r2e-security` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-security/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-security-v0.9.0/r2e-security/CHANGELOG.md)).
- Built against `r2e-static` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-static/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-static-v0.9.0/r2e-static/CHANGELOG.md)).
- Built against `r2e-tenant` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-tenant/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-tenant-v0.9.0/r2e-tenant/CHANGELOG.md)).
- Built against `r2e-utils` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-utils/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-utils-v0.9.0/r2e-utils/CHANGELOG.md)).

## [0.9.0] - 2026-10-25

### Changed

- Built against the `Routes` newtype of `r2e-core` / `r2e-http` (breaking
  there: `docs/migration/routes-newtype.md`); this crate's own API is
  unchanged.

- Built against `r2e-core` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.8.0/r2e-core/CHANGELOG.md)).
- Built against `r2e-macros` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-macros/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-macros-v0.8.0/r2e-macros/CHANGELOG.md)).
- Built against `r2e-static` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-static/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-static-v0.8.0/r2e-static/CHANGELOG.md)).
- Built against `r2e-data-diesel` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-data-diesel/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-data-diesel-v0.8.0/r2e-data/backends/diesel/CHANGELOG.md)).
- Built against `r2e-data-sqlx` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-data-sqlx/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-data-sqlx-v0.8.0/r2e-data/backends/sqlx/CHANGELOG.md)).
- Built against `r2e-events` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-events/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-events-v0.8.0/r2e-events/CHANGELOG.md)).
- Built against `r2e-events-iggy` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-events-iggy/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-events-iggy-v0.8.0/r2e-events/backends/iggy/CHANGELOG.md)).
- Built against `r2e-events-kafka` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-events-kafka/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-events-kafka-v0.8.0/r2e-events/backends/kafka/CHANGELOG.md)).
- Built against `r2e-events-pulsar` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-events-pulsar/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-events-pulsar-v0.8.0/r2e-events/backends/pulsar/CHANGELOG.md)).
- Built against `r2e-events-rabbitmq` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-events-rabbitmq/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-events-rabbitmq-v0.8.0/r2e-events/backends/rabbitmq/CHANGELOG.md)).
- Built against `r2e-executor` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-executor/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-executor-v0.8.0/r2e-executor/CHANGELOG.md)).
- Built against `r2e-grpc` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-grpc/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-grpc-v0.8.0/r2e-grpc/CHANGELOG.md)).
- Built against `r2e-mcp` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-mcp/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-mcp-v0.9.0/r2e-mcp/CHANGELOG.md)).
- Built against `r2e-observability` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-observability/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-observability-v0.8.0/r2e-observability/CHANGELOG.md)).
- Built against `r2e-oidc` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-oidc/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-oidc-v0.8.0/r2e-oidc/CHANGELOG.md)).
- Built against `r2e-openapi` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-openapi/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-openapi-v0.8.0/r2e-openapi/CHANGELOG.md)).
- Built against `r2e-openfga` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-openfga/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-openfga-v0.8.0/r2e-openfga/CHANGELOG.md)).
- Built against `r2e-prometheus` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-prometheus/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-prometheus-v0.8.0/r2e-prometheus/CHANGELOG.md)).
- Built against `r2e-rate-limit` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-rate-limit/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-rate-limit-v0.8.0/r2e-rate-limit/CHANGELOG.md)).
- Built against `r2e-scheduler` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-scheduler/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-scheduler-v0.8.0/r2e-scheduler/CHANGELOG.md)).
- Built against `r2e-security` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-security/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-security-v0.8.0/r2e-security/CHANGELOG.md)).
- Built against `r2e-tenant` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-tenant/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-tenant-v0.8.0/r2e-tenant/CHANGELOG.md)).
- Built against `r2e-utils` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-utils/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-utils-v0.8.0/r2e-utils/CHANGELOG.md)).

## [0.8.1] - 2026-10-15

### Fixed

- Depends on `r2e-core` 0.7.1 and `r2e-macros` 0.7.0: `r2e` 0.8.0 taken from
  crates.io without a `[patch]` failed to compile every `#[routes]` handler
  with validation and every `#[mcp_routes]` impl, because the published
  `r2e-macros` 0.6.0 predates the codegen those crates expect. The facade's
  own `r2e::r2e_macros` re-export now resolves to the same macro version as
  the prelude. No API change (#1092).

## [0.8.0] - 2026-10-12

### Changed

- Built against `r2e-core` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.7.0/r2e-core/CHANGELOG.md)).
- Built against `r2e-data-diesel` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-data-diesel/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-data-diesel-v0.7.0/r2e-data/backends/diesel/CHANGELOG.md)).
- Built against `r2e-data-sqlx` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-data-sqlx/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-data-sqlx-v0.7.0/r2e-data/backends/sqlx/CHANGELOG.md)).
- Built against `r2e-events` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-events/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-events-v0.7.0/r2e-events/CHANGELOG.md)).
- Built against `r2e-events-iggy` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-events-iggy/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-events-iggy-v0.7.0/r2e-events/backends/iggy/CHANGELOG.md)).
- Built against `r2e-events-kafka` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-events-kafka/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-events-kafka-v0.7.0/r2e-events/backends/kafka/CHANGELOG.md)).
- Built against `r2e-events-pulsar` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-events-pulsar/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-events-pulsar-v0.7.0/r2e-events/backends/pulsar/CHANGELOG.md)).
- Built against `r2e-events-rabbitmq` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-events-rabbitmq/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-events-rabbitmq-v0.7.0/r2e-events/backends/rabbitmq/CHANGELOG.md)).
- Built against `r2e-executor` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-executor/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-executor-v0.7.0/r2e-executor/CHANGELOG.md)).
- Built against `r2e-grpc` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-grpc/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-grpc-v0.7.0/r2e-grpc/CHANGELOG.md)).
- Built against `r2e-mcp` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-mcp/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-mcp-v0.8.0/r2e-mcp/CHANGELOG.md)).
- Built against `r2e-observability` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-observability/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-observability-v0.7.0/r2e-observability/CHANGELOG.md)).
- Built against `r2e-oidc` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-oidc/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-oidc-v0.7.0/r2e-oidc/CHANGELOG.md)).
- Built against `r2e-openapi` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-openapi/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-openapi-v0.7.0/r2e-openapi/CHANGELOG.md)).
- Built against `r2e-openfga` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-openfga/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-openfga-v0.7.0/r2e-openfga/CHANGELOG.md)).
- Built against `r2e-prometheus` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-prometheus/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-prometheus-v0.7.0/r2e-prometheus/CHANGELOG.md)).
- Built against `r2e-rate-limit` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-rate-limit/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-rate-limit-v0.7.0/r2e-rate-limit/CHANGELOG.md)).
- Built against `r2e-scheduler` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-scheduler/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-scheduler-v0.7.0/r2e-scheduler/CHANGELOG.md)).
- Built against `r2e-security` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-security/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-security-v0.7.0/r2e-security/CHANGELOG.md)).
- Built against `r2e-static` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-static/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-static-v0.7.0/r2e-static/CHANGELOG.md)).
- Built against `r2e-tenant` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-tenant/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-tenant-v0.7.0/r2e-tenant/CHANGELOG.md)).
- Built against `r2e-utils` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-utils/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-utils-v0.7.0/r2e-utils/CHANGELOG.md)).

## [0.7.0] - 2026-10-11

### Breaking

- `r2e::mcp` (feature `mcp`) re-exports `r2e-mcp` 0.7: the MCP endpoint is
  always mounted and every family always advertised, `McpServer::Provided` is
  a 4-tuple, `McpSessionError` → `McpMemberError` (alias kept), the route
  structs gain `meta`. Migration: [`docs/migration/mcp-registry.md`](../docs/migration/mcp-registry.md);
  details in [`r2e-mcp/CHANGELOG.md`](../r2e-mcp/CHANGELOG.md).

### Added

- `r2e::prelude` (feature `mcp`) re-exports `McpRegistry`, the `meta!` macro
  and `MetaObject`.

### Changed

- Built against `r2e-mcp` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-mcp/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-mcp-v0.7.0/r2e-mcp/CHANGELOG.md)).

