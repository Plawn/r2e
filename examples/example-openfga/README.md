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
- **Config-driven OpenFGA client**: two `#[producer]`s build the `GrpcBackend`
  and cached `OpenFgaRegistry` from `openfga.endpoint` / `openfga.store_id` /
  `openfga.model_id`.

## Running the tests (recommended)

The integration tests spin up a real OpenFGA server with `DevOpenFga`
(testcontainers), create a store, write the model + seed tuples, boot the app,
and exercise allowed/denied requests. They need Docker and are `#[ignore]`d by
default:

```bash
cargo test -p example-openfga --test openfga_test -- --ignored
```

No manual setup — the dev service owns the store/model/tuple bootstrap.

## Running the app standalone

Store and model IDs are server-generated, so they must be created before the
app boots. Start OpenFGA and bootstrap once:

```bash
docker compose up -d          # OpenFGA on :8080 (HTTP) and :8081 (gRPC)

# Create a store
STORE=$(curl -s -X POST http://localhost:8080/stores \
  -d '{"name":"documents"}' | jq -r .id)

# Write the model (the JSON below is authz::MODEL, generated from fga/model.fga)
MODEL=$(curl -s -X POST http://localhost:8080/stores/$STORE/authorization-models \
  -d '{"schema_version":"1.1","type_definitions":[
        {"type":"user"},
        {"type":"document","relations":{"viewer":{"this":{}},"editor":{"this":{}}},
         "metadata":{"relations":{
           "viewer":{"directly_related_user_types":[{"type":"user"}]},
           "editor":{"directly_related_user_types":[{"type":"user"}]}}}}]}' \
  | jq -r .authorization_model_id)

# Grant alice viewer + editor on document:readme
curl -s -X POST http://localhost:8080/stores/$STORE/write \
  -d "{\"authorization_model_id\":\"$MODEL\",\"writes\":{\"tuple_keys\":[
        {\"user\":\"user:alice\",\"relation\":\"viewer\",\"object\":\"document:readme\"},
        {\"user\":\"user:alice\",\"relation\":\"editor\",\"object\":\"document:readme\"}]}}"

echo "store_id=$STORE model_id=$MODEL"
```

Paste `store_id` / `model_id` into `application.yaml`, then run:

```bash
cargo run -p example-openfga
```

The app mints demo tokens via `example_openfga::demo_token("alice")`; use one as
a `Bearer` token:

```bash
TOKEN=... # from demo_token("alice")
curl -H "Authorization: Bearer $TOKEN" http://localhost:3000/documents/readme  # 200
curl http://localhost:3000/documents/readme                                     # 401
```
