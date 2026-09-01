---
topic: openfga
features: openfga
tokens: ~4000
requires: guards
---

## OpenFGA — fine-grained authorization (ReBAC)

### TL;DR

- Enable feature `openfga` and install the `OpenFga::model(authz::MODEL)`
  plugin: it owns the store/model lifecycle at boot and provides
  `OpenFgaRegistry`, `FgaClient` and `OpenFgaHandle`.
- Check the `.fga` model into the repo and generate the typed module with
  `model!(pub mod authz = "fga/model.fga")` — it is parsed and semantically
  validated at compile time, so a typo'd relation is a build error.
- Guard idiom: `#[guard(FgaCheck::has(authz::<type>::<relation>).from_path(path::<param>))]`;
  resolvers are `.from_path`, `.from_query`, `.from_header`, `.fixed`.
- The route must carry an identity (the subject is `user:{identity.sub()}`) —
  applying `FgaCheck` where the identity is statically always `None` is a
  compile error.
- Prefer `FgaCheck::has(...)` over the unchecked
  `FgaCheck::relation("…").on("…")` string form.
- In production set `apply_model: false` so a drift between the checked-in
  model and the live store fails startup instead of producing mystery 403s.
- Write tuples with `FgaClient::grant` / `revoke` (compile-checked by the
  model's `DirectlyAssignable` impls); they invalidate the cached decisions of
  the touched object.
- Build object ids with `authz::<type>::id("…")` — use `try_id` for anything
  coming from a request (it rejects `:`, `#`, `*` instead of panicking).
- There is no `FgaClient::list_objects`: paginate your own objects and `check`
  each, or call `backend.client().list_objects(...)` knowingly.
- Test without a server: `OpenFgaRegistry::new(MockBackend::new())` plus
  `mock.add_tuple(...)`.

This is the guard family for relationship-based authorization: `FgaCheck` is a
decorator spec applied with `#[guard(...)]` like any other (see llm/guards.md).

Requires feature `openfga` (crate `r2e-openfga`). Zanzibar-style
relationship checks: "does `user:<sub>` have `<relation>` on `<type>:<id>`?".
The `FgaCheck` guard runs post-auth, so the route **requires an identity**
(struct-level `#[inject(identity)]` or an identity handler param); the subject
is `user:{identity.sub()}`. Applying it where the identity is statically
always `None` — a controller with no `#[inject(identity)]`, or an
`#[anonymous]` route without an `Option<..>` identity param — is a **compile
error** (`FgaCheck` declares `DecoratorSpec::REQUIRES_IDENTITY = true`).
A required or `Option<..>` struct/param identity is allowed (runtime `None`
→ 401 stays as the backstop for the optional case).

**Setup — the `OpenFga` plugin (recommended).** One line owns the whole store
lifecycle and provides the beans (`OpenFgaRegistry` — the guard's only dep;
`FgaClient` — the typed client; `OpenFgaHandle` — resolved ids + raw backend):

```rust
# async fn __doc(b: AppBuilder) -> impl Sized {
use r2e::r2e_openfga::OpenFga;

r2e::r2e_openfga::model!(pub mod authz = "fga/model.fga");

b.load_config::<()>()
    .plugin(OpenFga::model(authz::MODEL))   // order vs load_config doesn't matter
    .build_state().await
    .register_controller::<DocumentController>()
# }
```

```yaml
openfga:
  endpoint: "http://localhost:8081"   # gRPC endpoint (required)
  store: "documents"                  # store name (looked up / created)
  # store_id: "01H…"                  # or an explicit id (wins over `store`)
  # apply_model: false                # prod: verify instead of apply
  # model_id: "01H…"                  # verify mode only: pin + verify this version
  # api_token, connect_timeout_secs (10), request_timeout_secs (5),
  # cache_enabled (true), cache_ttl_secs (60)
```

Boot sequence (inside `build_state()`, **before the app serves; any failure
aborts startup**): connect → resolve the store (`store_id`, or `store` name
via `ListStores`; created when missing in apply mode) → apply or verify the
model → pin the resolved `model_id` on every subsequent check.

- **Apply mode** (`apply_model: true`, the default — dev/test): the
  compiled-in `authz::MODEL` is structurally compared with the store's latest
  model; a new version is written **only when they differ** (FGA models are
  append-only; identical re-boots reuse the latest). Store names are not
  unique in OpenFGA — on duplicates the oldest is used (with a warning).
- **Verify mode** (`apply_model: false` — prod): the live model (latest, or
  the pinned `model_id`) must structurally match `authz::MODEL`, otherwise
  startup fails with a diff summary — no mystery 403s. Missing store /
  missing model are startup errors; duplicates by name are an error (set
  `store_id`).

This closes the schema-first chain: compile time checks code ↔ checked-in
`.fga` (via `model!`); the plugin checks checked-in `.fga` ↔ live store at
boot. `OpenFgaHandle` (bean) exposes `store_id()`, `model_id()`, and
`backend()` (the connected `GrpcBackend`, raw-client escape hatch; panics when
disabled — `try_backend()` is the non-panicking form). `openfga.enabled: false`
skips the boot sequence (checks then fail closed with `OpenFgaError::Disabled`).

**Manual setup (escape hatch — dynamic models / custom wiring).** Provide the
beans yourself; the store/model must already exist:

```rust
# async fn __doc() -> Result<(), Box<dyn std::error::Error>> { let _ = {
use r2e::r2e_openfga::{FgaClient, GrpcBackend, OpenFgaConfig, OpenFgaRegistry};

let config = OpenFgaConfig::new("http://localhost:8080", "store-id")
    .with_model_id("01H…")   // optional; latest model if omitted
    .with_api_token("secret") // optional Bearer token
    .with_cache(true, 60);    // decision cache TTL (seconds)
let backend = GrpcBackend::connect(&config).await?;
let registry = OpenFgaRegistry::with_cache(backend.clone(), config.cache_ttl_secs);

AppBuilder::new()
    .provide(FgaClient::new(registry.clone()))  // typed writes/checks
    .provide(registry)         // guard dep (cached check)
    .provide(backend)          // optional: raw client escape hatch
    .build_state().await
    .register_controller::<DocumentController>()
# }; Ok(()) }
```

`OpenFgaConfig` derives `Deserialize` (`endpoint`, `store_id`, `model_id?`,
`api_token?`, `cache_enabled`, `cache_ttl_secs`; builder-only:
`with_connect_timeout`, `with_request_timeout`, `without_cache`). Note the
plugin reads its own `OpenFgaPluginConfig` section (same `openfga:` prefix,
`store`/`apply_model` keys) — `OpenFgaConfig` is the manual-wiring type.

**Schema-first typed API (recommended)** — check the `.fga` model into the
repo and generate a typed module from it with `model!`. Relations and object
types used in code are then compile-checked against the model — a typo'd
relation is a build error, not a silent permanent 403:

```rust
// fga/model.fga (OpenFGA DSL, schema 1.1 — path relative to the crate root)
//   model
//     schema 1.1
//   type user
//   type document
//     relations
//       define viewer: [user]
//       define editor: [user]
r2e::r2e_openfga::model!(pub mod authz = "fga/model.fga");
// tests / tiny models: inline DSL instead of a path
r2e::r2e_openfga::model!(pub mod tiny_authz = inline r#"model
  schema 1.1
type user"#);
```

The `.fga` file is parsed AND semantically validated at compile time (unknown
relation/type/condition in the model = build error at the invocation, with the
`.fga` line). The generated module contains:

- `authz::MODEL: &str` — the model as schema 1.1 JSON (the
  `WriteAuthorizationModel` payload) for boot-time apply/verify;
- `authz::DSL: &str` — the embedded source (also makes edits to the `.fga`
  retrigger compilation);
- per type: `authz::document::Ty` (marker implementing `FgaType`: `NAME` plus
  `WILDCARD = Some("document:*")`, the wildcard's wire form as a compile-time
  literal — a hand-written `FgaType` impl may leave `WILDCARD` at its `None`
  default, and the string is then interned once per type on first use),
  `authz::document::id("readme")` → `FgaObject<Ty>` (formats `document:readme`,
  panics on `:` in the id; `try_id` is the fallible form for request input),
  `authz::document::wildcard()` → the `document:*` subject;
- per relation: `authz::document::viewer` — a `FgaRel<Ty, Viewer>` const
  (lowercase, like `path::doc_id`) carrying relation + object type;
  `authz::team::member.of(authz::team::id("eng"))` → the `team:eng#member`
  userset subject;
- `DirectlyAssignable<SubjectMarker>` impls mirroring the model's
  `directly_related_user_types` (subject markers: `user::Ty`,
  `(team::Ty, team::Member)`, `WildcardOf<user::Ty>`) —
  `FgaClient::grant`/`revoke` bound on them.

Grammar surface: full DSL 1.1 — `or`/`and`/`but not` (+ parentheses),
`X from Y`, `[user, user:*, team#member]`, `with <condition>` + `condition`
blocks (CEL passthrough). Modular models (schema 1.2 `module`/`extend`) are
rejected. The standalone parser is `r2e_openfga::model_parser`
(`parse`/`validate`, crate `r2e-openfga-model`, no proc-macro deps).

**Guard idiom** — `FgaCheck::has(authz::<type>::<relation>).<resolver>`; the
resolver supplies the object id:

```rust
use r2e::r2e_openfga::FgaCheck;

r2e::r2e_openfga::model!(pub mod authz = "fga/model.fga");   // or `use crate::authz;`

#[controller(path = "/documents")]
pub struct DocumentController {
    #[inject] documents: DocumentService,
    #[inject(identity)] user: AuthenticatedUser,   // subject = user:{sub}
}

#[routes]
impl DocumentController {
    // Everything checked at compile time: `authz::document::viewer` against
    // the .fga model, `path::doc_id` against the route's `{doc_id}`.
    #[get("/{doc_id}")]
    #[guard(FgaCheck::has(authz::document::viewer).from_path(path::doc_id))]
    async fn get(&self, Path(doc_id): Path<String>) -> Result<Json<Document>, HttpError> {
        Ok(Json(self.documents.load(&doc_id).await?))
    }

    #[put("/{doc_id}")]
    #[guard(FgaCheck::has(authz::document::editor).from_path(path::doc_id))]
    async fn update(&self, Path(doc_id): Path<String>, Json(b): Json<Update>)
        -> Result<Json<Document>, HttpError> {
        Ok(Json(self.documents.update(&doc_id, b).await?))
    }
}
# fn main() {}
```

`FgaCheck::relation("viewer").on("document")` is the **unchecked escape
hatch** for dynamic models (nothing verifies the strings) — prefer
`FgaCheck::has` whenever the model is checked in.

Resolvers: `.from_path(path::id | "id")` (route param), `.from_query("id")`
(query string), `.from_header("X-Document-Id")`, `.fixed("system:global")`
(pre-formatted `type:id` literal). Denied → 403, no identity → 401, missing/
unresolvable id → 400. **Security:** dynamic resolvers reject ids containing
`:`, `#`, or `*` (object-type / userset / wildcard injection guard) — so do
`id()`/`try_id()`; only `.fixed(...)` accepts a `type:id` string. The subject
side is guarded too: an `identity.sub()` containing a reserved character is
rejected with 403 (fail closed) rather than interpolated into `user:{sub}`.

**Typed client (`FgaClient`) — the idiomatic write path.** A clonable bean
(`FgaClient::new(registry)`) for everything handler-level. `grant`/`revoke`
compile only if the model's `directly_related_user_types` allows that subject
type on that relation (`DirectlyAssignable` bound), and invalidate the
decision cache for the touched object (write-through — the grantee's next
request sees the change). `check` goes through the registry (cached) and has
no assignability bound (checks may target computed relations):

```rust
use r2e::r2e_openfga::FgaClient;

r2e::r2e_openfga::model!(pub mod authz = "fga/model.fga");   // or `use crate::authz;`

#[controller(path = "/documents")]
pub struct DocumentController {
    #[inject] fga: FgaClient,
    #[inject(identity)] user: AuthenticatedUser,
}

#[routes]
impl DocumentController {
    #[post("/{doc_id}/share/{user_id}")]
    async fn share(&self, Path((doc_id, user_id)): Path<(String, String)>)
        -> Result<Json<bool>, HttpError> {
        // `InvalidObjectId` / `OpenFgaError` do NOT convert into `HttpError`:
        // map them explicitly (400 for bad input, 500 for a backend failure).
        let bad = |e: r2e::r2e_openfga::InvalidObjectId| HttpError::bad_request(e.to_string());
        let fail = |e: r2e::r2e_openfga::OpenFgaError| HttpError::internal(e.to_string());

        let grantee = authz::user::try_id(&user_id).map_err(bad)?;   // rejects `:`/`#`/`*` → 400
        let doc = authz::document::try_id(&doc_id).map_err(bad)?;
        self.fga.grant(&grantee, authz::document::viewer, &doc).await.map_err(fail)?;
        self.fga.revoke(&grantee, authz::document::viewer, &doc).await.map_err(fail)?;
        // userset / wildcard subjects, when the model allows them:
        self.fga.grant(&authz::team::member.of(authz::team::id("eng")), authz::document::viewer, &doc).await.map_err(fail)?;
        self.fga.grant(&authz::user::wildcard(), authz::document::viewer, &doc).await.map_err(fail)?;
        // handler-level check (cached):
        let allowed = self.fga.check(&grantee, authz::document::viewer, &doc).await.map_err(fail)?;
        Ok(Json(allowed))
    }
}
# fn main() {}
```

OpenFGA `Write` semantics apply: granting an existing tuple / revoking a
missing one is a server error, not a no-op. Only the exact object's cached
decisions are invalidated — after grants with transitive fan-out (e.g. into a
`team#member` used by many objects), use `registry.clear_cache()`.

There is **no `FgaClient::list_objects`** (deliberate): OpenFGA's
`ListObjects` response carries no truncation signal — the server-side bounds
(`OPENFGA_LIST_OBJECTS_MAX_RESULTS`, deadline) silently return a partial
list, so a typed wrapper would look exhaustive without being it. For
list-endpoint filtering, paginate your own objects and `check` each, or use
the raw client (`backend.client().list_objects(...)`) knowingly.

**Raw tuple writes (escape hatch)** — batch or conditional writes go through
the raw client; invalidate the cache manually:

```rust
# async fn __doc(backend: GrpcBackend, registry: OpenFgaRegistry) -> Result<(), Box<dyn std::error::Error>> {
use r2e::r2e_openfga::openfga_rs::{tonic, TupleKey, WriteRequest, WriteRequestWrites};
let mut client = backend.client().clone();
client.write(tonic::Request::new(WriteRequest {
    store_id: backend.store_id().into(),
    writes: Some(WriteRequestWrites { tuple_keys: vec![TupleKey {
        user: "user:alice".into(), relation: "viewer".into(),
        object: "document:1".into(), condition: None,
    }] }),
    ..Default::default()
})).await?;
registry.invalidate_object("document:1"); // or invalidate_user / clear_cache
# Ok(()) }
```

**Testing / no live server** — back the registry with `MockBackend` (direct
tuple lookup, no transitive eval) and seed tuples:

```rust
# fn __doc(builder: AppBuilder) -> impl Sized {
use r2e::r2e_openfga::{MockBackend, OpenFgaRegistry};
let mock = MockBackend::new();
mock.add_tuple("user:alice", "viewer", "document:readme");
// provide during app boot / test override:
builder.provide(OpenFgaRegistry::new(mock))
# }
```

`MockBackend` implements the full backend surface (check + tuple writes), so
`FgaClient` works against it in tests — clones share the tuple set
(`mock.clone()` keeps a seeding/asserting handle).

Custom backends: implement the `OpenFgaBackend` trait (`check` is required;
`write_tuple`/`delete_tuple` default to `OpenFgaError::Unsupported`) and wrap
it in `OpenFgaRegistry::new(..)` / `::with_cache(.., ttl)`.

### Do not

- Do not treat writes as idempotent: granting an existing tuple or revoking a
  missing one is a server error, not a no-op.
- Do not assume a grant with transitive fan-out (e.g. into a `team#member`
  used by many objects) clears the right cache entries — only the exact
  object's decisions are invalidated; call `registry.clear_cache()`.
- Do not interpolate request input into a `type:id` string: use
  `try_id(...)`; only `.fixed(...)` accepts a pre-formatted `type:id`.
- Do not use modular models (schema 1.2 `module`/`extend`) — `model!` rejects
  them.
