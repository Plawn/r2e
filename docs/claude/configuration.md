# Configuration Reference

## R2eConfig

Central configuration store. `R2eConfig<()>` = raw key-value; `R2eConfig<T: ConfigProperties>` = adds typed access via `Deref<Target = T>`.

### Constructors (on `R2eConfig<()>` only)

- `R2eConfig::load("dev")` — load `application.yaml` + `application-{profile}.yaml` + `.env` + `.env.{profile}` + env vars. Profile overridable via `R2E_PROFILE`.
- `R2eConfig::load_with_resolver("prod", &resolver)` — same but with custom `SecretResolver`.
- `R2eConfig::from_yaml_str(yaml, profile)` — parse a YAML string (testing).
- `R2eConfig::empty()` — empty config (testing).

### Methods (on all `R2eConfig<T>`)

- `get::<V: FromConfigValue>("key")` → `Result<V, ConfigError>` — typed retrieval.
- `get_or("key", default)` → `V` — with fallback.
- `contains_key("key")` → `bool`.
- `profile()` → `&str` — active profile name.
- `typed()` → `&T` — reference to typed layer.
- `raw()` → `R2eConfig<()>` — downgrade to untyped.

### Methods (on `R2eConfig<()>` only)

- `set("key", ConfigValue::...)` — insert/overwrite.
- `with_typed::<C: ConfigProperties>(prefix)` → `Result<R2eConfig<C>, ConfigError>` — upgrade to typed. `prefix` is `Option<&str>`.

## Resolution order (lowest → highest priority)

1. `application.yaml`
2. `application-{profile}.yaml`
3. `.env` file (loaded via dotenvy, won't overwrite existing env vars)
4. `.env.{profile}` file
5. `${...}` secret placeholders resolved in string values
6. Environment variables (`APP_DATABASE_URL` ↔ `app.database.url`)

Profile: `R2E_PROFILE` env var > `load()` argument > default `"dev"`.

## ConfigValue

```rust
enum ConfigValue {
    String(String), Integer(i64), Float(f64), Bool(bool), Null,
    List(Vec<ConfigValue>), Map(HashMap<String, ConfigValue>),
}
```

YAML hierarchies are flattened to dot-separated keys. Sequences stored as `List` at parent key AND individually (`key.0`, `key.1`).

## FromConfigValue

Converts `ConfigValue` to Rust types. Built-in impls: `String`, `i64`, `f64`, `bool`, `i8`/`i16`/`i32`, `u8`/`u16`/`u32`/`u64`/`usize`, `f32`, `Option<T>`, `Vec<T>`, `HashMap<String, V>`.

## Secrets

Syntax in YAML string values:
- `${VAR}` → env var
- `${env:VAR}` → explicit env var
- `${file:/path}` → file read (trimmed)

`SecretResolver` trait: `fn resolve(&self, reference: &str) -> Result<String, ConfigError>`.
`DefaultSecretResolver` handles `env:` and `file:` prefixes, falls back to env var.

## ConfigProperties (typed sections)

Derive macro: `#[derive(ConfigProperties)]`. No struct-level attribute required — the prefix is a runtime parameter.

Field attributes:
- `#[config(default = value)]` — fallback if key missing
- `#[config(key = "nested.key")]` — override key path relative to prefix
- `#[config(env = "VAR")]` — explicit env var
- `#[config(section)]` — nested `ConfigProperties` type (delegates to sub-prefix)
- `Option<T>` field type — always safe, `None` if missing
- Doc comments → `PropertyMeta::description` (shown in validation errors)

Generated trait methods:
- `properties_metadata(prefix: Option<&str>) -> Vec<PropertyMeta>`
- `from_config(config, prefix: Option<&str>) -> Result<Self, ConfigError>`

### PropertyMeta

```rust
struct PropertyMeta {
    key: String,             // relative
    full_key: String,        // absolute (computed from runtime prefix)
    type_name: &'static str,
    required: bool,
    default_value: Option<String>,
    description: Option<String>,
    env_var: Option<String>,
    is_section: bool,
}
```

## Injection

### In controllers

- `#[config("app.key")] field: T` — raw key, `T: FromConfigValue`, resolved per-request from state
- `#[config_section(prefix = "app")] field: C` — `C: ConfigProperties`, loaded per-request with given prefix

### In beans/producers

- `#[config("key")] param: T` on `#[bean]` / `#[producer]` constructor parameters

## Startup validation

`AppBuilder::register_controller` calls `C::validate_config(config)`:
- Checks all `#[config("key")]` fields exist
- Calls `validate_section::<T>(config, Some(prefix))` for `#[config_section(prefix = "...")]` fields
- Panics with formatted error listing missing keys, expected types, env var hints, and descriptions

`validate_keys(config, &[("source", "key", "type")])` → `Vec<MissingKeyError>` — manual validation.
`validate_section::<C>(config, prefix)` → `Vec<MissingKeyError>` — validates a ConfigProperties section.

## Registry

`register_section::<C: ConfigProperties>(prefix)` — global registry for introspection. `prefix` is `Option<&str>`.
`registered_sections()` → `Vec<RegisteredSection { prefix, properties }>`.

## ConfigError

```rust
enum ConfigError {
    NotFound(String),
    TypeMismatch { key: String, expected: &'static str },
    Load(String),
    Validation(Vec<ConfigValidationDetail>),
}
```

## Prelude exports

`R2eConfig`, `ConfigProperties`, `ConfigValue`, `ConfigError`, `ConfigValidationDetail`, `FromConfigValue`, `SecretResolver`, `DefaultSecretResolver`.
