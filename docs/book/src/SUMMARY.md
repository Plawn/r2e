# Summary

[Introduction](./introduction.md)

# Getting Started

- [Installation](./getting-started/installation.md)
- [Your First App](./getting-started/first-app.md)
- [CLI Scaffolding](./getting-started/cli-scaffolding.md)
- [Project Structure](./getting-started/project-structure.md)

# Core Concepts

- [Controllers](./core-concepts/controllers.md)
- [Dependency Injection](./core-concepts/dependency-injection.md)
- [State and Beans](./core-concepts/state-and-beans.md)
- [Plugins](./core-concepts/plugins.md)
- [Configuration](./core-concepts/configuration.md)
- [Error Handling](./core-concepts/error-handling.md)
- [Validation](./core-concepts/validation.md)
- [Live Configuration](./core-concepts/live-config.md)

# Security

- [JWT / OIDC Authentication](./security/jwt-oidc.md)
- [Embedded OIDC Server](./security/embedded-oidc.md)
- [Optional Identity](./security/optional-identity.md)
- [Guards and Roles](./security/guards-and-roles.md)
- [Rate Limiting](./security/rate-limiting.md)
- [Multi-Tenancy](./security/multi-tenancy.md)
- [OpenFGA Authorization](./security/openfga.md)

# Data Access

- [Pagination](./data-access/pagination.md)
- [Managed Database Transactions](./data-access/transactions.md)

# Events and Scheduling

- [Event Bus](./events-and-scheduling/event-bus.md)
- [Declarative Consumers](./events-and-scheduling/consumers.md)
- [Scheduling](./events-and-scheduling/scheduling.md)
- [Dynamic Scheduled Tasks](./events-and-scheduling/dynamic-scheduled-tasks.md)
- [Executor and Background Services](./events-and-scheduling/background-services.md)

# Real-Time

- [Server-Sent Events (SSE)](./real-time/sse.md)
- [WebSocket](./real-time/websocket.md)

# gRPC

- [gRPC Services](./grpc/services.md)
- [Guards and Identity](./grpc/guards-and-identity.md)

# MCP

- [MCP Server](./mcp/server.md)

# Advanced

- [Interceptors](./advanced/interceptors.md)
- [Custom Guards](./advanced/custom-guards.md)
- [Custom Plugins](./advanced/custom-plugins.md)
- [Macro Debugging](./advanced/macro-debugging.md)
- [Managed Resources](./advanced/managed-resources.md)
- [Lifecycle Hooks](./advanced/lifecycle-hooks.md)
- [Dev Mode](./advanced/dev-mode.md)
- [Route Attributes](./advanced/route-attributes.md)
- [Catch-All and Proxy Routes](./advanced/catch-all-routes.md)
- [OpenAPI](./advanced/openapi.md)
- [Health Checks](./advanced/health-checks.md)
- [Secure Headers](./advanced/secure-headers.md)
- [Static Files](./advanced/static-files.md)
- [Multipart File Uploads](./advanced/multipart.md)
- [Observability](./advanced/observability.md)
- [Prometheus Metrics](./advanced/prometheus-metrics.md)
- [Caching](./advanced/caching.md)
- [Serve Lifecycle and Graceful Shutdown](./advanced/serve-lifecycle.md)
- [Sharded Serving](./advanced/sharded-serving.md)
- [QUIC and HTTP/3](./advanced/quic-http3.md)
- [Performance Guide](./advanced/performance.md)
- [Controller Lifecycle and Handler Dispatch](./advanced/controller-lifecycle-and-dispatch.md)

# Testing

- [Test Setup](./testing/test-setup.md)
- [TestApp](./testing/test-app.md)
- [TestJwt](./testing/test-jwt.md)
- [TestSession](./testing/test-session.md)
- [Integration Patterns](./testing/integration-patterns.md)

# Reference

- [Crate Map](./reference/crate-map.md)
- [CLI Reference](./reference/cli-reference.md)
- [API Documentation](./reference/api-docs.md)
- [Migration 0.4 → 0.5: error projection](./reference/migration-0.5.md)
- [Migration 0.5 → 0.6: body schemas](./reference/migration-0.6.md)
- [Migration: Generic DI builder bounds](./reference/migrations/di-provision-index.md)
- [Migration: gRPC identity rides the HTTP extractor contract (r2e-grpc 0.9 → 0.10)](./reference/migrations/grpc-identity.md)
- [Migration: Layer bands — LayerSlot (r2e-core 0.9 → 0.10)](./reference/migrations/layer-slots.md)
- [Migration: MCP mount stage and identity contract (r2e-mcp 0.9 → 0.10)](./reference/migrations/mcp-identity.md)
- [Migration: MCP global registry (r2e-mcp 0.6 → 0.7)](./reference/migrations/mcp-registry.md)
- [Migration: Observability contributes to R2E's subscriber](./reference/migrations/observability-tracing.md)
- [Migration: The plugin API](./reference/migrations/plugin-api.md)
- [Migration: Rejection layout (r2e-core 0.6 → 0.7)](./reference/migrations/rejection-layout.md)
- [Migration: Routes replaces axum::Router (r2e-core 0.7 → 0.8)](./reference/migrations/routes-newtype.md)
- [Migration: One tracing path through the builder (r2e-core 0.9)](./reference/migrations/tracing-single-path.md)
