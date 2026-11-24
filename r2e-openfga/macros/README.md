# r2e-openfga-macros

Proc macros for [`r2e-openfga`](../README.md): `model!` turns a checked-in `.fga` file into a typed authorization API at compile time.

## Overview

The `.fga` model is the single source of truth. `model!` parses and semantically validates it at compile time (with [`r2e-openfga-model`](../model/README.md)); errors point at the macro invocation with the offending `.fga` line in the message. A typo in a relation name is a build error instead of a silent permanent 403.

## Usage

Applications use the macro through `r2e-openfga`, which re-exports it — do not depend on this crate directly:

```toml
[dependencies]
r2e = { version = "0.15", features = ["openfga"] }
```

```rust
r2e::r2e_openfga::model!(pub mod authz = "fga/model.fga");
```

The path is relative to the crate root (`CARGO_MANIFEST_DIR`). For tests and tiny models the DSL can be given inline:

```rust
r2e::r2e_openfga::model!(pub mod authz = inline r#"
model
  schema 1.1
type user
"#);
```

### Generated items

- `authz::MODEL` — the schema 1.1 JSON of the model (for boot-time apply/verify with `OpenFga::model(authz::MODEL)`);
- per type: `authz::<type>::Ty` (an `FgaType` marker) and `authz::<type>::id("x")` → `FgaObject<Ty>` (formats `type:x`, rejects `:` in the id);
- per relation: `authz::<type>::<relation>` — an `FgaRel<Ty, Marker>` const usable with `FgaCheck::has(...)`;
- `DirectlyAssignable` impls encoding `directly_related_user_types`, so typed writes (`FgaClient::grant`/`revoke`) check subject types at compile time.

```rust
#[get("/{doc_id}")]
#[guard(FgaCheck::has(authz::document::viewer).from_path(path::doc_id))]
async fn view(&self, Path(doc_id): Path<String>) -> Json<Doc> { /* ... */ }
```

## Documentation

Full guide: [`docs/features/23-openfga.md`](../../docs/features/23-openfga.md).

## License

Apache-2.0
