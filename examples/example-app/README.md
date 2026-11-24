# example-app

The reference demo: one application that exercises most of R2E in a single
process. It is also the integration-test target for the framework (the binary
and `tests/` boot the same `App` type from `src/app.rs`).

What it shows:

- Feature modules (`UserModule`, `OrderModule`) registered with `#[module]`
- Controllers for users, orders, events, `data/users` (managed SQLx
  transactions on in-memory SQLite), identity, mixed public/protected routes,
  reports, problem details, a catch-all proxy, SSE, a WebSocket echo
  (`/ws/echo`), notifications, documents (OpenFGA checks against a mock
  backend), multipart uploads, account and live config
- JWT security with roles; a demo HS256 token (`sub = user-123`) is printed at
  startup
- Plugins: `Scheduler`, `Executor`, `Prometheus` (`/metrics`), `Health`,
  `RequestIdPlugin`, `SecureHeaders`, `Cors`, `Observability`, `OpenApiPlugin`
  (`/docs`), `NormalizePath`
- `error_projection::<AppError>()`, an EventBus-to-SSE bridge, a 30 s timeout
  layer, `on_start` / `on_stop` hooks

## Running

```bash
cargo run -p example-app --bin example-app
```

The server listens on `http://0.0.0.0:3001` (`application.yaml`). Copy the token
printed at startup and call a protected route:

```bash
curl -s localhost:3001/users -H "Authorization: Bearer <token>"
```

OpenAPI docs are at `http://localhost:3001/docs`, metrics at `/metrics`.

`--bin example-app` is needed because the crate has two more binaries,
`boot_failure` and `boot_failure_graph`: deliberately failing boots driven by
`tests/app/boot_failure.rs`.

## Testing

```bash
cargo test -p example-app
```

Tests are grouped by subsystem under `tests/` (`app`, `events`, `hotpath`,
`http`, `scheduling`, `security`, `transverse`). They boot the real app with
`TestApp` and the `application-test.yaml` profile.
