# Feature 23 — OpenFGA Authorization (ReBAC, schema-first)

## TL;DR

Zanzibar-style relationship-based access control via [OpenFGA](https://openfga.dev/), schema-first: the `.fga` model checked into the repo is the single source of truth. `model!(pub mod authz = "fga/model.fga")` parses and validates it at **compile time** and generates a typed API — guards are declared as `#[guard(FgaCheck::has(authz::document::viewer).from_path(path::doc_id))]`, where a typo'd relation is a build error (with a did-you-mean) instead of a silent permanent 403. `authz::MODEL` carries the schema 1.1 JSON for store bootstrap. Requires feature `openfga`; setup is two beans (`GrpcBackend` + `OpenFgaRegistry`), no plugin yet. Dynamic resolvers and `id()`/`try_id()` reject the FGA metacharacters `:`/`#`/`*` (injection guard), and so does the identity subject.

## Objective

Fine-grained, relationship-based authorization ("does `user:alice` have `viewer` on `document:readme`?") with the same compile-time safety as the rest of R2E: relations, object types, and assignable subject types are all checked against the authorization model at build time.

## Feature Flag

```toml
r2e = { features = ["openfga"] }
```

Included in `full`. Crates: `r2e-openfga` (runtime + guard), `r2e-openfga-model` (standalone `.fga` parser, no proc-macro deps), `r2e-openfga-macros` (`model!`).

## Core Concepts

### The model is the source of truth

Write the authorization model in the OpenFGA DSL and check it in:

```text
# fga/model.fga
model
  schema 1.1

type user

type team
  relations
    define member: [user]

type document
  relations
    define parent: [document]
    define viewer: [user, user:*, team#member] or viewer from parent
    define editor: [user] and viewer
```

Then generate the typed API (path is relative to the crate root):

```rust
r2e::r2e_openfga::model!(pub mod authz = "fga/model.fga");
```

The file is parsed **and semantically validated at compile time**: unknown types, relations (`[team#membr]`), or conditions in the model fail the build at the invocation with the offending `.fga` line. Editing the `.fga` retriggers compilation (the source is embedded via `include_str!`).

The full DSL 1.1 grammar is supported: `or` / `and` / `but not` (n-ary, with parentheses; mixing without parentheses is rejected), tuple-to-userset (`viewer from parent`), direct restrictions (`[user, user:*, team#member]`), and `condition` blocks (CEL passthrough with typed parameters, including `list<>` / `map<>`). Modular models (schema 1.2 `module` / `extend`) are rejected with a clear error.

For tests and tiny models, the DSL can be inlined: `model!(pub mod authz = inline r#"..."#)`.

### The generated module

| Item | Meaning |
|---|---|
| `authz::MODEL: &str` | The model as schema 1.1 JSON — the `WriteAuthorizationModel` payload for store bootstrap / boot-time verify |
| `authz::DSL: &str` | The embedded `.fga` source |
| `authz::document::Ty` | Type marker (implements `FgaType`; `NAME = "document"`) |
| `authz::document::id("readme")` | `FgaObject<Ty>` formatting `document:readme` — panics on `:`/`#`/`*`; `try_id` is the fallible form for request-supplied input |
| `authz::document::viewer` | `FgaRel<Ty, Viewer>` const (lowercase, same convention as `path::doc_id`) carrying relation + object type |
| `authz::team::member.of(authz::team::id("eng"))` | The `team:eng#member` userset subject |
| `authz::user::wildcard()` | The `user:*` public subject |
| `impl DirectlyAssignable<…> for Viewer` | One impl per entry in the relation's `directly_related_user_types` — the typed write API (Phase 2) bounds grants on it |

### Guards

```rust
use r2e::prelude::*;
use r2e::r2e_openfga::FgaCheck;
use crate::authz;

#[controller(path = "/documents")]
pub struct DocumentController {
    #[inject(identity)] user: AuthenticatedUser,   // subject = user:{sub}
}

#[routes]
impl DocumentController {
    // Compile-checked twice over: the relation against fga/model.fga,
    // the path param against the route's `{doc_id}` placeholder.
    #[get("/{doc_id}")]
    #[guard(FgaCheck::has(authz::document::viewer).from_path(path::doc_id))]
    async fn view(&self, Path(doc_id): Path<String>) -> Json<Doc> { ... }
}
```

Resolvers supply the object id: `.from_path(path::name | "name")`, `.from_query("id")`, `.from_header("X-Document-Id")`, `.fixed("system:global")`. Responses: denied → 403, no identity → 401, unresolvable id → 400.

`FgaCheck::relation("viewer").on("document")` remains as the **unchecked escape hatch** for dynamic models — nothing verifies the strings; prefer `has` whenever the model is checked in.

An FGA check requires an authenticated identity (`REQUIRES_IDENTITY = true`): placing one where the identity is statically always `None` (no `#[inject(identity)]`, or an `#[anonymous]` route without an optional identity param) is a **compile error**.

### Injection guards (security)

FGA metacharacters are rejected on both sides of a check, fail-closed:

- **Object ids** from dynamic resolvers and `id()`/`try_id()` must not contain `:` (type injection: `secret:admin`), `#` (userset reference), or `*` (wildcard). Only `.fixed(...)` accepts a pre-formatted `type:id` literal.
- **The identity subject**: if `identity.sub()` contains `:`/`#`/`*` the check is rejected with 403 before `user:{sub}` is formed — a forged `sub = "*"` must never collapse onto public-wildcard grants.

## Setup

Two beans, no plugin (a boot-time apply/verify plugin is planned — see Roadmap below):

```rust
use r2e::r2e_openfga::{GrpcBackend, OpenFgaConfig, OpenFgaRegistry};

#[producer]
async fn openfga_backend(
    #[config("openfga.endpoint")] endpoint: String,
    #[config("openfga.store_id")] store_id: String,
    #[config("openfga.model_id")] model_id: Option<String>,
) -> GrpcBackend {
    let mut config = OpenFgaConfig::new(endpoint, store_id);
    if let Some(model_id) = model_id { config = config.with_model_id(model_id); }
    GrpcBackend::connect(&config).await.expect("OpenFGA unreachable")
}

#[producer]
async fn openfga_registry(backend: GrpcBackend) -> OpenFgaRegistry {
    OpenFgaRegistry::with_cache(backend, 60)   // decision cache, 60s TTL
}
```

The `FgaCheck` guard pulls the `OpenFgaRegistry` bean itself (compile-checked decorator dep) — controllers need no `#[inject]` field for it. Keep `GrpcBackend` provided too if you write tuples (raw client access; remember `registry.invalidate_object(...)` after writes).

## Testing

- **Unit / no server** — back the registry with `MockBackend` (direct tuple lookup) and pin it: `builder.override_bean(OpenFgaRegistry::new(mock))`.
- **Integration** — `DevOpenFga` (r2e-devservices, feature `openfga`) runs a real server via testcontainers and owns the store/model bootstrap; write `authz::MODEL` so tests exercise the exact model the guards were compile-checked against:

```rust
let fga = DevOpenFga::shared().await;
let store_id = fga.create_store("documents").await;
let model_id = fga.write_model(&store_id, &serde_json::from_str(authz::MODEL)?).await;
fga.write_tuples(&store_id, &model_id, &[("user:alice", "viewer", "document:readme")]).await;
TestApp::boot_with::<App>(move |b| {
    b.override_config_value("openfga.endpoint", fga.grpc_endpoint().to_string())
        .override_config_value("openfga.store_id", store_id)
        .override_config_value("openfga.model_id", model_id)
}).await
```

See `examples/example-openfga` for the complete wiring, and `r2e-compile-tests/compile-{pass,fail}/fga_*.rs` for what is (and is not) accepted at compile time.

## Standalone parser

`r2e_openfga::model_parser` (crate `r2e-openfga-model`) exposes the `.fga` parser without proc-macro machinery — usable from build scripts and tooling: `parse(dsl)` (syntax, corpus-exact vs the official transformer) and `validate(&model)` (semantic referential checks), plus the serde data model for schema 1.1 JSON in both directions.

## Roadmap (not yet shipped)

- **Typed client (Phase 2)** — `fga.grant(user, authz::document::viewer, obj)` / `revoke` / `list_objects`, compile-bounded by `DirectlyAssignable`, with write-through cache invalidation.
- **Plugin (Phase 3)** — `.with(OpenFga::model(authz::MODEL))`: apply the model at boot in dev/test, *verify* against the live store in prod (mismatch = startup error), pin the resolved `model_id`.
- **CLI (Phase 4)** — `r2e fga diff | push | pull`, tuple seed fixtures.
