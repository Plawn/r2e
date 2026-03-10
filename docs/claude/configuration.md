# Configuration Reference

## Idiomatic Usage Guide

There are **two distinct mechanisms** for injecting configuration into controllers. Choose based on how many values you need:

### Approach 1: Individual values with `#[config("key")]`

Use when you need **1-2 isolated values**. The field type must implement `FromConfigValue` (scalar types: `String`, `i64`, `f64`, `bool`, `Option<T>`, `Vec<T>`, etc.).

```rust
#[derive(Controller)]
#[controller(state = Services)]
pub struct GreetController {
    #[config("app.name")]
    name: String,                    // required — panics at startup if missing

    #[config("app.max_retries")]
    max_retries: Option<i64>,        // optional — None if missing
}
```

The key is a **full dot-separated path** into the YAML. With this YAML:
```yaml
app:
  name: "my-app"
  max_retries: 3
```
`"app.name"` resolves to `"my-app"`, `"app.max_retries"` resolves to `3`.

### Approach 2: Typed config section with `#[config_section]` (recommended for structured config)

Use when you have a **group of related settings** (database, oidc, app settings, etc.). This is the idiomatic approach for anything beyond 1-2 values.

**Step 1:** Define a struct deriving `ConfigProperties`:
```rust
#[derive(ConfigProperties, Clone, Debug)]
pub struct AppConfig {
    /// Application name
    pub name: String,

    /// Welcome greeting
    #[config(default = "Hello!")]
    pub greeting: String,

    /// Application version
    pub version: Option<String>,
}
```

**Step 2:** Inject it into the controller with `#[config_section(prefix = "...")]`:
```rust
#[derive(Controller)]
#[controller(state = Services)]
pub struct ConfigController {
    #[config_section(prefix = "app")]
    app_config: AppConfig,
}

#[routes]
impl ConfigController {
    #[get("/config")]
    async fn config_info(&self) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "name": self.app_config.name,
            "greeting": self.app_config.greeting,
            "version": self.app_config.version,
        }))
    }
}
```

**Step 3:** Provide the matching YAML:
```yaml
app:
  name: "my-app"
  greeting: "Welcome!"
  # version is Option, omitting it is fine
```

The `prefix` tells the macro where to look in the YAML hierarchy. Each field is resolved as `prefix + "." + field_name`, so `prefix = "app"` + field `name` → key `"app.name"`.

### How to choose

| Situation | Use |
|---|---|
| 1-2 isolated values from different sections | `#[config("full.key")]` |
| A coherent group of settings (db, auth, app...) | `#[config_section(prefix = "...")]` |
| Config needed outside controllers (main, beans) | `ConfigProperties::from_config()` |
| Config section as injectable bean | `#[config_section(prefix = "...")]` in beans/producers |

### Outside controllers: manual usage

In `main()`, services, or tests — use `ConfigProperties::from_config()` directly:
```rust
let config = R2eConfig::load()?;
let db = DatabaseConfig::from_config(&config, Some("app.database"))?;
println!("url = {}", db.url);
```

### Providing config to AppBuilder

There are **two ways** to provide configuration to the builder (both are pre-state methods):

#### `with_config(config)` — provide a pre-loaded config

Use when you already have an `R2eConfig` instance (hot-reload, tests, custom loading):

```rust
let config = R2eConfig::load().unwrap_or_else(|_| R2eConfig::empty());
AppBuilder::new()
    .with_config(config)
    .build_state::<Services, _, _>().await
```

#### `load_config::<C>()` — load + type + provide in one call

Use for the common case where you just want to load from YAML files:

```rust
// Raw config only:
AppBuilder::new()
    .load_config::<()>()

// With typed config struct:
AppBuilder::new()
    .load_config::<AppConfig>()
```

`C` must implement `LoadableConfig` — satisfied by `()` (raw only) and any `T: ConfigProperties` (raw + typed). Panics if loading or typed construction fails.

Both methods do the same thing under the hood:
1. Store the raw config in the builder (for `serve_auto`, etc.)
2. Provide `R2eConfig` in the bean registry (injectable by beans via `#[config("key")]` and `#[config_section(prefix = "...")]`)
3. If `C` is not `()`, also construct and provide the typed config via `ConfigProperties::from_config()`

**Important:** `with_config` / `load_config` are **pre-state** methods — call them before `.build_state()` or `.with_state()`.

### `#[config_section(prefix = "...")]` — config section in beans and producers

Config sections can be injected directly into beans, producers, and `#[derive(Bean)]` structs using `#[config_section(prefix = "...")]` — the same syntax as in controllers. This replaces the former `with_config_section` builder method.

In a `#[bean]` impl:
```rust
#[bean]
impl SearchService {
    fn new(
        #[config_section(prefix = "cve.matching")] matching: CveMatchingConfig,
        other_dep: OtherDep,
    ) -> Self { ... }
}
```

With `#[derive(Bean)]`:
```rust
#[derive(Clone, Bean)]
struct SearchService {
    #[config_section(prefix = "cve.matching")]
    matching: CveMatchingConfig,
}
```

In a `#[producer]`:
```rust
#[producer]
fn create_search(#[config_section(prefix = "cve.matching")] matching: CveMatchingConfig) -> SearchService { ... }
```

The struct must derive `ConfigProperties`. Field-level `#[config(default)]`, `#[config(env)]`, etc. are respected.

---

## R2eConfig

Central configuration store. Not generic — stores flat key-value pairs.

### Constructors

- `R2eConfig::load()` — load `application.yaml` + `.env` + env vars.
- `R2eConfig::load_with_resolver(&resolver)` — same but with custom `SecretResolver`.
- `R2eConfig::from_yaml_str(yaml)` — parse a YAML string (testing).
- `R2eConfig::empty()` — empty config (testing).

### Methods

- `get::<V: FromConfigValue>("key")` → `Result<V, ConfigError>` — typed retrieval.
- `get_or("key", default)` → `V` — with fallback.
- `contains_key("key")` → `bool`.
- `set("key", ConfigValue::...)` — insert/overwrite.

## Resolution order (lowest → highest priority)

1. `application.yaml`
2. `.env` file (loaded via dotenvy, won't overwrite existing env vars)
3. `${...}` secret placeholders resolved in string values
4. Environment variables (`APP_DATABASE_URL` ↔ `app.database.url`)

## ConfigValue

```rust
enum ConfigValue {
    String(String), Integer(i64), Float(f64), Bool(bool), Null,
    List(Vec<ConfigValue>), Map(HashMap<String, ConfigValue>),
}
```

YAML hierarchies are flattened to dot-separated keys. Sequences stored as `List` at parent key AND individually (`key.0`, `key.1`).

## FromConfigValue

Converts `ConfigValue` to Rust types. Built-in impls: `String`, `PathBuf`, `i64`, `f64`, `bool`, `i8`/`i16`/`i32`, `u8`/`u16`/`u32`/`u64`/`usize`, `f32`, `Option<T>`, `Vec<T>`, `HashMap<String, V>`.

## Secrets

Syntax in YAML string values:
- `${VAR}` → env var
- `${env:VAR}` → explicit env var
- `${file:/path}` → file read (trimmed)

`SecretResolver` trait: `fn resolve(&self, reference: &str) -> Result<String, ConfigError>`.
`DefaultSecretResolver` handles `env:` and `file:` prefixes, falls back to env var.

## ConfigProperties — defining typed config structs

`#[derive(ConfigProperties)]` generates an impl of the `ConfigProperties` trait, which knows how to read fields from `R2eConfig` using a runtime prefix.

**There is no struct-level prefix attribute.** The prefix is always provided at the call site (controller's `#[config_section(prefix = "...")]`, or `from_config(&config, Some("prefix"))` in code).

### Field resolution rule

Each field resolves to the key: **`prefix + "." + field_name`** (or `field_name` alone if prefix is `None`).

Example: with `prefix = "app.database"` and field `pool_size` → reads key `"app.database.pool_size"`.

### Field attributes

| Attribute | Effect | Example |
|---|---|---|
| *(none)* | Required scalar. Panics/errors if missing. | `pub url: String` |
| `Option<T>` | Optional scalar. `None` if missing. | `pub timeout: Option<i64>` |
| `#[config(default = value)]` | Fallback value if key is missing. | `#[config(default = 10)] pub pool_size: i64` |
| `#[config(default = "str")]` | String default (auto `.into()`). | `#[config(default = "hello")] pub greeting: String` |
| `#[config(key = "custom.path")]` | Override the key path (relative to prefix). | `#[config(key = "jwks.url")] pub jwks_url: String` → reads `prefix.jwks.url` instead of `prefix.jwks_url` |
| `#[config(env = "VAR")]` | Explicit env var fallback if key is missing. YAML still takes priority. | `#[config(env = "DATABASE_URL")] pub url: String` |
| `#[config(section)]` | **Nested sub-struct.** See below. | `#[config(section)] pub database: DatabaseConfig` |
| `/// doc comment` | Description shown in validation error messages. | `/// Connection timeout in seconds` |

Attributes can be combined: `#[config(key = "client.id", default = "my-app")]`.

### `#[config(section)]` — nested ConfigProperties

**Why it exists:** The derive macro operates on tokens only — it cannot resolve traits at compile time. It cannot tell whether a field type implements `FromConfigValue` (scalar) or `ConfigProperties` (nested struct). `#[config(section)]` explicitly tells the macro to generate a recursive `from_config()` call instead of a scalar `get()` call.

**What it does:** Instead of `config.get::<T>(key)`, it generates:
```rust
DatabaseConfig::from_config(&config, Some("app.database"))
//                                         ^ prefix + "." + field_name
```

**Full example:**

```rust
#[derive(ConfigProperties, Clone, Debug)]
pub struct DatabaseConfig {
    pub url: String,
    #[config(default = 5)]
    pub pool_size: i64,
}

#[derive(ConfigProperties, Clone, Debug)]
pub struct TlsConfig {
    pub cert: String,
    pub key: String,
}

#[derive(ConfigProperties, Clone, Debug)]
pub struct AppConfig {
    pub name: String,
    #[config(section)]
    pub database: DatabaseConfig,       // required section
    #[config(section)]
    pub tls: Option<TlsConfig>,         // optional section — None if absent
}
```

```yaml
app:
  name: "my-app"
  database:
    url: "postgres://localhost/mydb"
    pool_size: 20
  # tls section omitted → tls = None
```

Resolution with `prefix = "app"`:
- `app.name` → `"my-app"`
- `app.database` → delegates to `DatabaseConfig::from_config(&config, Some("app.database"))`
  - `app.database.url` → `"postgres://localhost/mydb"`
  - `app.database.pool_size` → `20`
- `app.tls` → delegates to `TlsConfig::from_config(...)`, but section is absent → `None`

**Without `#[config(section)]`**, the macro would generate `config.get::<DatabaseConfig>("app.database")` which fails because `DatabaseConfig` does not implement `FromConfigValue`.

### `#[config(key = "...")]` — custom key override

Useful when the YAML hierarchy doesn't match the Rust field name:

```rust
#[derive(ConfigProperties, Clone, Debug)]
pub struct OidcConfig {
    pub issuer: Option<String>,
    #[config(key = "jwks.url")]
    pub jwks_url: Option<String>,       // reads prefix.jwks.url (not prefix.jwks_url)
    #[config(key = "client.id", default = "my-app")]
    pub client_id: String,              // reads prefix.client.id
}
```

With `prefix = "oidc"`:
- `oidc.issuer` → normal
- `oidc.jwks.url` → because of `key = "jwks.url"` (instead of default `oidc.jwks_url`)
- `oidc.client.id` → because of `key = "client.id"`

### `#[config(env = "...")]` — explicit env var fallback

Priority: **YAML value > env var > default > error/None**.

```rust
#[derive(ConfigProperties, Clone, Debug)]
pub struct DbConfig {
    #[config(env = "DATABASE_URL")]
    pub url: String,                    // if not in YAML, tries env var DATABASE_URL
    #[config(default = 5)]
    pub pool_size: i64,
}
```

### Generated trait methods

- `properties_metadata(prefix: Option<&str>) -> Vec<PropertyMeta>` — introspection metadata for all fields.
- `from_config(config: &R2eConfig, prefix: Option<&str>) -> Result<Self, ConfigError>` — construct the struct from config values.

### PropertyMeta

```rust
struct PropertyMeta {
    key: String,             // relative (e.g., "pool_size")
    full_key: String,        // absolute (e.g., "app.database.pool_size")
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

Two attributes, two different mechanisms:

- **`#[config("app.key")] field: T`** — reads a single scalar value. `T` must implement `FromConfigValue`. The key is a full dot-separated path.
- **`#[config_section(prefix = "app")] field: C`** — reads an entire typed section. `C` must implement `ConfigProperties` (via `#[derive(ConfigProperties)]`). The prefix determines where to look in the YAML hierarchy.

Both are resolved per-request from the `R2eConfig` stored in Axum state.

### In beans/producers

Both attributes work on `#[bean]` / `#[producer]` constructor parameters and `#[derive(Bean)]` struct fields:

- **`#[config("key")] param: T`** — single scalar value. Same as in controllers.
- **`#[config_section(prefix = "...")] param: C`** — typed config section. `C` must implement `ConfigProperties`.

```rust
#[bean]
impl SearchService {
    fn new(
        #[config("app.name")] name: String,
        #[config_section(prefix = "cve.matching")] matching: MatchingConfig,
        other_dep: OtherDep,
    ) -> Self { ... }
}
```

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
