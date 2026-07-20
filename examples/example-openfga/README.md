# example-openfga

Fine-grained authorization with [OpenFGA](https://openfga.dev/) — Zanzibar-style
relationship-based access control wired into R2E controllers via `#[guard]`.

## What it shows

- **Schema-first authorization**: the model lives in `fga/model.fga` (OpenFGA
  DSL); `model!(pub mod authz = "fga/model.fga")` in `app.rs` generates a
  typed API from it at compile time. `authz::MODEL` is the schema 1.1 JSON
  the tests write to the store — code and store share one source of truth.
- **Typed `FgaCheck` guards** on a `DocumentController`:
  - `GET /documents/{doc_id}` requires `authz::document::viewer`
  - `PUT /documents/{doc_id}` requires `authz::document::editor`
  - A typo'd relation is a compile error, not a silent 403. Object IDs are
    resolved from the `{doc_id}` path parameter (`path::doc_id`, also
    compile-checked) and checked as `document:{doc_id}`; the caller is
    `user:{sub}` from the JWT identity.
- **Plugin-owned store lifecycle**: `.plugin(OpenFga::model(authz::MODEL))`
  connects to `openfga.endpoint`, looks up the `openfga.store` store by name
  (creating it when missing), applies the compiled-in model when it differs
  from the store's latest, pins the resolved model id for every check, and
  provides the `OpenFgaRegistry` / `FgaClient` / `OpenFgaHandle` beans. In
  production, `openfga.apply_model: false` switches to verify mode: a model
  mismatch fails startup instead of surfacing as mystery 403s.

## Running the tests (recommended)

The integration tests spin up a real OpenFGA server with `DevOpenFga`
(testcontainers) and point the app at it — the plugin creates a per-test store
and applies the model at boot; the tests seed tuples through the typed
`FgaClient` bean and exercise allowed/denied requests. They need Docker and
are `#[ignore]`d by default:

```bash
cargo test -p example-openfga --test openfga_test -- --ignored
```

No manual setup — the plugin owns the store/model bootstrap.

## Running the app standalone

Start OpenFGA and run — the plugin creates the `documents` store and applies
the model at boot:

```bash
docker compose up -d          # OpenFGA on :8080 (HTTP) and :8081 (gRPC)
cargo run -p example-openfga
```

The app mints demo tokens via `example_openfga::demo_token("alice")`; use one as
a `Bearer` token:

```bash
TOKEN=... # from demo_token("alice")
curl -H "Authorization: Bearer $TOKEN" http://localhost:3000/documents/readme  # 200
curl http://localhost:3000/documents/readme                                     # 401
```
