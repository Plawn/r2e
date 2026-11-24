# OpenFGA Authorization

R2E integrates [OpenFGA](https://openfga.dev/) for relationship-based access
control ("does `user:alice` have `viewer` on `document:readme`?"). It is
schema-first: the `.fga` model in your repository is the source of truth,
checked at compile time and verified against the live store at boot.

## Setup

```toml
r2e = { version = "0.15", features = ["openfga"] }
```

`openfga` is **not** part of `full`. The feature re-exports `r2e::r2e_openfga`
and adds its types (`model!`, `FgaCheck`, `FgaClient`, `OpenFga`,
`OpenFgaHandle`, `OpenFgaRegistry`) to the prelude.

## The model

Write the model in the OpenFGA DSL and check it in:

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

Generate a typed API from it (the path is relative to the crate root):

```rust
r2e::r2e_openfga::model!(pub mod authz = "fga/model.fga");
```

The model is parsed and validated at compile time. An unknown type or relation
fails the build with the offending `.fga` line. For tests and small models, use
`model!(pub mod authz = inline r#"..."#)`.

The generated module contains:

| Item | Meaning |
|---|---|
| `authz::MODEL` | The model as JSON, applied or verified at boot |
| `authz::document::viewer` | A typed relation constant |
| `authz::document::id("readme")` | The object `document:readme` (panics on `:`, `#`, `*`) |
| `authz::document::try_id(s)` | Fallible form, for request input |
| `authz::team::member.of(authz::team::id("eng"))` | The userset `team:eng#member` |
| `authz::user::wildcard()` | The public subject `user:*` |

## Guarding routes

```rust
use r2e::prelude::*;
use crate::authz;

#[controller(path = "/documents")]
pub struct DocumentController {
    #[inject(identity)] user: AuthenticatedUser, // subject = user:{sub}
}

#[routes]
impl DocumentController {
    #[get("/{doc_id}")]
    #[guard(FgaCheck::has(authz::document::viewer).from_path(path::doc_id))]
    async fn view(&self, Path(doc_id): Path<String>) -> Json<Doc> {
        // ...
    }
}
```

A misspelled relation or path parameter is a compile error. Object ids come
from `.from_path(..)`, `.from_query("id")`, `.from_header("X-Document-Id")` or
`.fixed("system:global")`.

| Outcome | Status |
|---|---|
| Relation denied | 403 |
| No identity | 401 |
| Object id cannot be resolved | 400 |

The guard rejects `:`, `#` and `*` in object ids and in the identity's `sub`,
so a forged id cannot reach another type or a wildcard grant.

## Writes and handler-level checks

Inject `FgaClient` for grants, revokes, and checks on objects that are only
known after a lookup:

```rust
#[controller(path = "/documents")]
pub struct ShareController {
    #[inject] fga: FgaClient,
    #[inject(identity)] user: AuthenticatedUser,
}

// inside a handler:
let grantee = authz::user::try_id(&user_id)?;
let doc = authz::document::try_id(&doc_id)?;
self.fga.grant(&grantee, authz::document::viewer, &doc).await?;
let allowed = self.fga.check(&grantee, authz::document::viewer, &doc).await?;
```

`grant` and `revoke` only compile for subject types the model allows on that
relation, and they invalidate the decision cache.

## Plugin and configuration

```rust
b.load_config::<()>()
    .plugin(OpenFga::model(authz::MODEL))
    .build_state()
    .await
    .register_controller::<DocumentController>()
```

The plugin provides `OpenFgaRegistry`, `FgaClient` and `OpenFgaHandle`. During
`build_state()` it connects, resolves the store, then applies or verifies the
model. Any failure aborts startup.

```yaml
openfga:
  endpoint: "http://localhost:8081"  # gRPC endpoint (required)
  store: "documents"                 # store name, created if missing (apply mode)
  # store_id: "01H..."               # explicit id, wins over `store`
  # apply_model: false               # production: verify instead of apply
  # model_id: "01H..."               # verify mode: pin this version
```

| Key | Default | Meaning |
|---|---|---|
| `openfga.enabled` | `true` | `false` skips boot; checks fail closed |
| `openfga.apply_model` | `true` | Write the model when it differs; `false` verifies only |
| `openfga.api_token` | — | Bearer token for the OpenFGA server |
| `openfga.connect_timeout_secs` | `10` | Connection timeout |
| `openfga.request_timeout_secs` | `5` | Per-request timeout |
| `openfga.cache_enabled` | `true` | Decision cache |
| `openfga.cache_ttl_secs` | `60` | Decision cache TTL |

## Testing

- **No server**: back the registry with `MockBackend` and pin it with
  `override_bean(OpenFgaRegistry::new(mock))`. Pin all three provided beans
  (`OpenFgaRegistry`, `FgaClient`, `OpenFgaHandle`) to skip the gRPC boot, or
  set `openfga.enabled: false`.
- **Real server**: `DevOpenFga` from `r2e-devservices` (feature `openfga`) runs
  OpenFGA in a container:

```rust
let fga = DevOpenFga::shared().await;
let grpc = fga.grpc_endpoint().to_string();
let app = TestApp::boot_with::<App>(move |b| {
    b.override_config_value("openfga.endpoint", grpc)
        .override_config_value("openfga.store", "documents-test")
}).await;
```

See `examples/example-openfga` for the complete wiring.

## See also

- Full guide: [docs/features/23-openfga.md](https://github.com/plawn/r2e/blob/master/docs/features/23-openfga.md)
- [Guards and Roles](./guards-and-roles.md)
