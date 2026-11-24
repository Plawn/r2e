# r2e-openfga-model

Parser and validator for the OpenFGA authorization-model DSL (schema 1.1).

## Overview

Turns a `.fga` file into an `AuthorizationModel` whose JSON serialization matches the official `openfga/language` transformer byte-for-byte (validated against its vendored DSL↔JSON test corpus). It has no proc-macro dependencies, so it can be used standalone, from build scripts, or from the `model!` macro of [`r2e-openfga`](../README.md).

Two layers, matching the official tooling:

- `parse` — **syntax only** (structure, operator mixing, duplicates).
- `validate` — **semantic** referential checks (unknown types, relations, conditions).

The `model!` macro runs both; call `validate` yourself when using the parser standalone.

Conditions are CEL passthrough: parameter lists are typed, the expression body is embedded verbatim in the JSON. Modular models (schema 1.2, `module`/`extend`) are rejected with a clear error.

## Usage

Most applications never depend on this crate: `r2e-openfga` (facade feature `openfga`) uses it through `model!` and re-exports it as `r2e_openfga::model_parser`. For standalone use:

```toml
[dependencies]
r2e-openfga-model = "0.6"
```

```rust
let model = r2e_openfga_model::parse(r#"
model
  schema 1.1

type user

type document
  relations
    define viewer: [user]
    define editor: [user] or viewer
"#.trim_start()).unwrap();

r2e_openfga_model::validate(&model).unwrap();
let json = model.to_json();
assert_eq!(json["schema_version"], "1.1");
```

## Key types

| Item | Description |
|------|-------------|
| `parse` / `ParseError` | DSL → `AuthorizationModel` (syntax) |
| `validate` / `ValidationError` | Semantic checks on a parsed model |
| `AuthorizationModel` | Parsed model; `to_json()` gives the schema 1.1 JSON |
| `TypeDefinition`, `Userset`, `RelationReference`, `Condition`, … | Model AST |

## License

Apache-2.0
