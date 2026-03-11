# Configuration

## Overview

R2E configuration flows through three layers:

1. **R2eConfig** — flat key-value store, loaded from YAML + env
2. **ConfigProperties** — typed structs derived from R2eConfig
3. **Injection** — `#[config]` and `#[config_section]` in controllers, beans, and producers

---

## R2eConfig

Central configuration store. Flat dot-separated keys → `ConfigValue`.

### Loading

```rust
R2eConfig::load()                          // application.yaml + .env + env vars
R2eConfig::load_with_resolver(&resolver)   // custom SecretResolver
R2eConfig::from_yaml_str(yaml)             // testing
R2eConfig::empty()                         // testing
```

### Resolution order (lowest → highest priority)

1. `application.yaml` (hierarchies flattened: `app.database.url`)
2. `.env` file (via dotenvy, won't overwrite existing env vars)
3. `${...}` secret placeholders resolved in string values
4. Environment variables (`APP_DATABASE_URL` ↔ `app.database.url`)

### Methods

| Method | Returns | Description |
|---|---|---|
| `get::<V>("key")` | `Result<V, ConfigError>` | Typed retrieval. `V: FromConfigValue` |
| `get_or("key", default)` | `V` | With fallback |
| `contains_key("key")` | `bool` | Key existence |
| `set("key", ConfigValue::...)` | `()` | Insert/overwrite |

---

## ConfigValue & FromConfigValue

```rust
enum ConfigValue {
    String(String), Integer(i64), Float(f64), Bool(bool), Null,
    List(Vec<ConfigValue>), Map(HashMap<String, ConfigValue>),
}
```

YAML hierarchies are flattened to dot-separated keys. Sequences stored as `List` at parent key AND individually (`key.0`, `key.1`).

`FromConfigValue` converts `ConfigValue` to Rust types.
Built-in impls: `String`, `PathBuf`, `i64`, `f64`, `bool`, `i8`–`i32`, `u8`–`u64`, `usize`, `f32`, `Option<T>`, `Vec<T>`, `HashMap<String, V>`.

---

## Secrets

Syntax in YAML string values:

| Pattern | Source |
|---|---|
| `${VAR}` | Env var (shorthand) |
| `${VAR:default}` | Env var with fallback value |
| `${env:VAR}` | Env var (explicit) |
| `${env:VAR:default}` | Explicit env var with fallback |
| `${file:/path}` | File contents (trimmed) |

Custom resolvers implement `SecretResolver`: `fn resolve(&self, reference: &str) -> Result<String, ConfigError>`.

---

## ConfigProperties — typed config structs

`#[derive(ConfigProperties)]` generates a struct that reads its fields from `R2eConfig` using a runtime prefix.

**There is no struct-level prefix attribute.** The prefix is always provided at the injection site or call site.

### Field resolution

Each field resolves to: **`prefix + "." + field_name`** (or `field_name` alone if prefix is `None`).

### Field attributes

| Attribute | Effect | Example |
|---|---|---|
| *(none)* | Required. Error if missing. | `pub url: String` |
| `Option<T>` | Optional. `None` if missing. | `pub timeout: Option<i64>` |
| `#[config(default = value)]` | Fallback if missing | `#[config(default = 10)] pub pool_size: i64` |
| `#[config(default = "str")]` | String default (auto `.into()`) | `#[config(default = "hello")] pub greeting: String` |
| `#[config(key = "custom.path")]` | Override key (relative to prefix) | `#[config(key = "jwks.url")] pub jwks_url: String` |
| `#[config(env = "VAR")]` | Env var fallback if key missing | `#[config(env = "DATABASE_URL")] pub url: String` |
| `#[config(section)]` | Nested sub-struct (recursive `from_config()`) | `#[config(section)] pub database: DatabaseConfig` |
| `/// doc comment` | Description in validation errors | `/// Connection timeout` |

Attributes combine: `#[config(key = "client.id", default = "my-app")]`.

Priority for a field: **YAML > env var (`#[config(env)]`) > default > error/None**.

### Why `#[config(section)]` is required

The derive macro operates on tokens only — it cannot resolve traits. It cannot tell whether a field implements `FromConfigValue` (scalar) or `ConfigProperties` (nested struct). `#[config(section)]` tells the macro to generate `T::from_config(...)` instead of `config.get::<T>(...)`.

### Example: nested sections

```rust
#[derive(ConfigProperties, Clone, Debug)]
pub struct DatabaseConfig {
    pub url: String,
    #[config(default = 5)]
    pub pool_size: i64,
}

#[derive(ConfigProperties, Clone, Debug)]
pub struct AppConfig {
    pub name: String,
    #[config(section)]
    pub database: DatabaseConfig,         // required section
    #[config(section)]
    pub tls: Option<TlsConfig>,           // optional section — None if absent
}
```

```yaml
app:
  name: "my-app"
  database:
    url: "postgres://localhost/mydb"
    pool_size: 20
  # tls omitted → None
```

With `prefix = "app"`: `app.name` → scalar, `app.database` → delegates to `DatabaseConfig::from_config(&config, Some("app.database"))`.

### Manual usage (outside injection)

```rust
let config = R2eConfig::load()?;
let db = DatabaseConfig::from_config(&config, Some("app.database"))?;
```

### Generated trait

```rust
trait ConfigProperties {
    fn from_config(config: &R2eConfig, prefix: Option<&str>) -> Result<Self, ConfigError>;
    fn properties_metadata(prefix: Option<&str>) -> Vec<PropertyMeta>;
}
```

---

## Injection

Both `#[config]` and `#[config_section]` work **identically** in controllers, beans, and producers.

### `#[config("key")]` — single value

Reads one scalar from `R2eConfig`. The key is a full dot-separated path. Type must implement `FromConfigValue`.

### `#[config_section(prefix = "...")]` — typed section

Reads an entire struct from `R2eConfig`. Type must implement `ConfigProperties` (via `#[derive(ConfigProperties)]`).

### When to use which

| Situation | Use |
|---|---|
| 1–2 isolated values from different sections | `#[config("full.key")]` |
| Coherent group of settings | `#[config_section(prefix = "...")]` |
| Config needed outside DI (main, tests) | `ConfigProperties::from_config()` |

### In controllers

```rust
#[derive(Controller)]
#[controller(state = Services)]
pub struct MyController {
    #[config("app.name")]
    name: String,

    #[config("app.max_retries")]
    max_retries: Option<i64>,

    #[config_section(prefix = "app")]
    app_config: AppConfig,
}
```

### In beans and producers

```rust
#[bean]
impl SearchService {
    fn new(
        #[config("app.name")] name: String,
        #[config_section(prefix = "cve.matching")] matching: MatchingConfig,
        other_dep: OtherDep,
    ) -> Self { ... }
}

#[derive(Clone, Bean)]
struct SearchService {
    #[config_section(prefix = "cve.matching")]
    matching: MatchingConfig,
}

#[producer]
fn create_search(#[config_section(prefix = "cve.matching")] m: MatchingConfig) -> SearchService { ... }
```

---

## AppBuilder integration

Two pre-state methods to provide config (call before `.build_state()` or `.with_state()`):

### `load_config::<C>()` — load + provide (recommended)

The idiomatic way to set up configuration. Loads YAML + env, stores the raw config in the builder, and provides `R2eConfig` in the bean registry. If `C` is not `()`, also constructs and provides the typed config.

```rust
AppBuilder::new().load_config::<()>()           // raw config only
AppBuilder::new().load_config::<AppConfig>()    // raw + typed (preferred)
```

`C` must implement `LoadableConfig` — satisfied by `()` (raw only) and any `T: ConfigProperties`.

### `with_config(config)` — provide pre-loaded

Only needed when you have a pre-loaded `R2eConfig` (hot-reload, custom loading, tests). Prefer `load_config` in all other cases.

```rust
let config = R2eConfig::load().unwrap_or_else(|_| R2eConfig::empty());
AppBuilder::new().with_config(config)
```

---

## Validation

`AppBuilder::register_controller` calls `C::validate_config(config)`:
- Checks all `#[config("key")]` fields exist
- Calls `validate_section::<T>(config, Some(prefix))` for `#[config_section]` fields
- Panics with formatted error listing missing keys, expected types, env var hints, descriptions

Manual: `validate_keys(config, &[("source", "key", "type")])` → `Vec<MissingKeyError>`.

---

## Reference

### ConfigError

```rust
enum ConfigError {
    NotFound(String),
    TypeMismatch { key: String, expected: &'static str },
    Load(String),
    Validation(Vec<ConfigValidationDetail>),
}
```

### PropertyMeta

```rust
struct PropertyMeta {
    key: String,              // relative ("pool_size")
    full_key: String,         // absolute ("app.database.pool_size")
    type_name: &'static str,
    required: bool,
    default_value: Option<String>,
    description: Option<String>,
    env_var: Option<String>,
    is_section: bool,
}
```

### Registry

`register_section::<C: ConfigProperties>(prefix)` — global registry for introspection.
`registered_sections()` → `Vec<RegisteredSection { prefix, properties }>`.

### Prelude exports

`R2eConfig`, `ConfigProperties`, `ConfigValue`, `ConfigError`, `ConfigValidationDetail`, `FromConfigValue`, `SecretResolver`, `DefaultSecretResolver`.
