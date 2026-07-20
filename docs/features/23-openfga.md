# Feature 23 — OpenFGA Authorization (ReBAC, schema-first)

## TL;DR

Zanzibar-style relationship-based access control via [OpenFGA](https://openfga.dev/), schema-first: the `.fga` model checked into the repo is the single source of truth. `model!(pub mod authz = "fga/model.fga")` parses and validates it at **compile time** and generates a typed API — guards are declared as `#[guard(FgaCheck::has(authz::document::viewer).from_path(path::doc_id))]`, where a typo'd relation is a build error (with a did-you-mean) instead of a silent permanent 403. In handlers, `FgaClient` is the typed client: `grant`/`revoke` compile only for subject types the model allows (`DirectlyAssignable`) and invalidate the decision cache write-through; `check` covers handler-level checks. (No `list_objects` — OpenFGA cannot signal truncation, see below.) Setup is one line: `.plugin(OpenFga::model(authz::MODEL))` owns the store lifecycle at boot — ensure/create the store, apply the model when it differs (dev/test), or *verify* the live store matches and fail startup otherwise (prod, `openfga.apply_model: false`), pinning the resolved `model_id` for all checks. Requires feature `openfga`. Dynamic resolvers and `id()`/`try_id()` reject the FGA metacharacters `:`/`#`/`*` (injection guard), and so does the identity subject.

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
| `authz::MODEL: &str` | The model as schema 1.1 JSON — what `OpenFga::model(...)` applies/verifies at boot (the `WriteAuthorizationModel` payload) |
| `authz::DSL: &str` | The embedded `.fga` source |
| `authz::document::Ty` | Type marker (implements `FgaType`; `NAME = "document"`) |
| `authz::document::id("readme")` | `FgaObject<Ty>` formatting `document:readme` — panics on `:`/`#`/`*`; `try_id` is the fallible form for request-supplied input |
| `authz::document::viewer` | `FgaRel<Ty, Viewer>` const (lowercase, same convention as `path::doc_id`) carrying relation + object type |
| `authz::team::member.of(authz::team::id("eng"))` | The `team:eng#member` userset subject |
| `authz::user::wildcard()` | The `user:*` public subject |
| `impl DirectlyAssignable<…> for Viewer` | One impl per entry in the relation's `directly_related_user_types` — `FgaClient::grant`/`revoke` bound on it |

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

### The typed client (`FgaClient`)

Guards cover route-level checks; `FgaClient` covers everything handler-level — writes, and checks on objects known only after a DB lookup:

```rust
#[controller(path = "/documents")]
pub struct DocumentController {
    #[inject] fga: FgaClient,
    #[inject(identity)] user: AuthenticatedUser,
}

// Typed grant: compiles only because the model lists `user` in viewer's
// directly-related types; invalidates the decision cache for the document.
let grantee = authz::user::try_id(&user_id)?;          // 400 on `:`/`#`/`*`
let doc = authz::document::try_id(&doc_id)?;
self.fga.grant(&grantee, authz::document::viewer, &doc).await?;

// Userset / wildcard subjects, when the model allows them:
self.fga.grant(&authz::team::member.of(authz::team::id("eng")), authz::document::viewer, &doc).await?;
self.fga.grant(&authz::user::wildcard(), authz::document::viewer, &doc).await?;

// Handler-level check (cached via the registry). No DirectlyAssignable
// bound — checks may target computed relations (`viewer` implied by `editor`).
let allowed = self.fga.check(&grantee, authz::document::viewer, &doc).await?;
```

Semantics to know:

- **Write-through invalidation** — `grant`/`revoke` invalidate cached decisions for the touched object, so the grantee's next request sees the change immediately. Only the *exact object* is invalidated; grants with transitive fan-out (e.g. into a `team#member` used by many objects) still need `registry.clear_cache()` or TTL expiry. A concurrent check racing the write can re-cache the pre-write decision until TTL (invalidate-after-write is not versioned) — the cache TTL is the staleness bound either way.
- **OpenFGA `Write` semantics** — granting an existing tuple / revoking a missing one is a server error, not a no-op.
- **Wrong subject type = compile error** — `grant(&team_member_userset, authz::document::editor, …)` fails to build when `editor` only allows `[user]`.
- **Escape hatch** — batch or conditional writes go through `GrpcBackend::client()` (raw tonic client) + manual `registry.invalidate_object(...)`.
- `OpenFgaBackend` gained default-erroring `write_tuple`/`delete_tuple` — custom check-only backends still compile and surface `OpenFgaError::Unsupported` if used with `FgaClient`; `MockBackend` implements both, so `FgaClient` is fully testable offline.
- **No `list_objects` (deliberate)** — OpenFGA's `ListObjects` response is a bare `repeated string objects`: the server-side bounds (`OPENFGA_LIST_OBJECTS_MAX_RESULTS`, deadline) silently return a *partial* list with no truncation flag or cursor, so a typed wrapper would look exhaustive without being it. For list-endpoint filtering, paginate your own objects and `check` them (a future `BatchCheck`-based helper is the candidate), or call `backend.client().list_objects(...)` knowingly.

An FGA check requires an authenticated identity (`REQUIRES_IDENTITY = true`): placing one where the identity is statically always `None` (no `#[inject(identity)]`, or an `#[anonymous]` route without an optional identity param) is a **compile error**.

### Injection guards (security)

FGA metacharacters are rejected on both sides of a check, fail-closed:

- **Object ids** from dynamic resolvers and `id()`/`try_id()` must not contain `:` (type injection: `secret:admin`), `#` (userset reference), or `*` (wildcard). Only `.fixed(...)` accepts a pre-formatted `type:id` literal.
- **The identity subject**: if `identity.sub()` contains `:`/`#`/`*` the check is rejected with 403 before `user:{sub}` is formed — a forged `sub = "*"` must never collapse onto public-wildcard grants.

## Setup — the `OpenFga` plugin

One line; the plugin provides the beans (`OpenFgaRegistry`, `FgaClient`, `OpenFgaHandle`) and owns the store lifecycle:

```rust
use r2e::r2e_openfga::OpenFga;

r2e::r2e_openfga::model!(pub mod authz = "fga/model.fga");

b.load_config::<()>()                       // config must be loaded first
    .plugin(OpenFga::model(authz::MODEL))
    .build_state().await
    .register_controller::<DocumentController>()
```

```yaml
openfga:
  endpoint: "http://localhost:8081"   # gRPC endpoint (required)
  store: "documents"                  # store name — looked up, created if missing
  # store_id: "01H…"                  # or an explicit id (wins over `store`)
  # apply_model: false                # prod: verify instead of apply
  # model_id: "01H…"                  # verify mode only: pin + verify this version
  # api_token, connect_timeout_secs (10), request_timeout_secs (5),
  # cache_enabled (true), cache_ttl_secs (60)
```

The boot sequence runs inside `build_state()`, before the app serves; **any failure aborts startup**:

1. Connect to the gRPC endpoint.
2. Resolve the store — explicit `store_id`, or `store` name via `ListStores`. Apply mode creates a missing store; verify mode errors. (Store names are not unique in OpenFGA: apply mode uses the oldest duplicate with a warning; verify mode refuses to guess — set `store_id`.)
3. Apply or verify the model:
   - **Apply mode** (`apply_model: true`, default — dev/test): the compiled-in `authz::MODEL` is structurally compared with the store's latest model; a new version is written **only when they differ** (models are append-only — identical re-boots reuse the latest version).
   - **Verify mode** (`apply_model: false` — prod): the live model (latest, or the configured `model_id`) must structurally match, otherwise startup fails with a diff summary — fail-fast instead of mystery 403s.
4. Pin the resolved `model_id`: every check runs against one model version for the lifetime of the deploy.

This closes the schema-first chain: compile time checks code ↔ checked-in `.fga`; boot checks checked-in `.fga` ↔ live store. Structural comparison ignores server-side noise (model id, `module`/`source_info` annotations, empty-vs-absent metadata) — the machinery is public in `r2e_openfga::model_convert` (`compile_model`, `models_equal`, `diff_summary`).

The `FgaCheck` guard pulls the `OpenFgaRegistry` bean itself (compile-checked decorator dep) — controllers need no `#[inject]` field for it. `#[inject] fga: FgaClient` is for handlers doing writes/checks. `OpenFgaHandle` exposes the resolved `store_id()` / `model_id()` and `backend()` — the connected `GrpcBackend` for the raw-client escape hatch (batch/conditional writes, model management). Install the plugin **after** `load_config()`/`with_config()` (it panics with guidance otherwise); `openfga.enabled: false` skips the boot sequence entirely (checks then fail closed with `OpenFgaError::NotReady`).

**Manual wiring (escape hatch)** — for dynamic models or custom backends, provide the beans yourself exactly as before: `GrpcBackend::connect(&OpenFgaConfig::new(endpoint, store_id)).await` → `OpenFgaRegistry::with_cache(backend, ttl)` → `FgaClient::new(registry)`. The store/model must then already exist (nothing applies or verifies them).

## Testing

- **Unit / no server** — back the registry with `MockBackend` (direct tuple lookup) and pin it: `builder.override_bean(OpenFgaRegistry::new(mock))`.
- **Integration** — `DevOpenFga` (r2e-devservices, feature `openfga`) runs a real server via testcontainers; the plugin does the store/model bootstrap, so a test only injects the endpoint (plus a unique store name for isolation on the session-shared container) and seeds tuples through the typed client:

```rust
let fga = DevOpenFga::shared().await;
let grpc = fga.grpc_endpoint().to_string();
let app = TestApp::boot_with::<App>(move |b| {
    b.override_config_value("openfga.endpoint", grpc)
        .override_config_value("openfga.store", format!("documents-{}", test_id))
}).await;
let client = app.bean::<FgaClient>();
client.grant(&authz::user::id("alice"), authz::document::viewer, &authz::document::id("readme")).await?;
```

(For non-plugin wiring, `DevOpenFga` keeps the HTTP bootstrap helpers: `create_store`, `write_model`, `write_tuples`.)

See `examples/example-openfga` for the complete wiring, `r2e-openfga/tests/plugin.rs` for the lifecycle behaviors (apply/reuse/append/verify), and `r2e-compile-tests/compile-{pass,fail}/fga_*.rs` for what is (and is not) accepted at compile time.

## Standalone parser

`r2e_openfga::model_parser` (crate `r2e-openfga-model`) exposes the `.fga` parser without proc-macro machinery — usable from build scripts and tooling: `parse(dsl)` (syntax, corpus-exact vs the official transformer) and `validate(&model)` (semantic referential checks), plus the serde data model for schema 1.1 JSON in both directions.

## Roadmap (not yet shipped)

- **CLI (Phase 4)** — `r2e fga diff | push | pull`, tuple seed fixtures.
