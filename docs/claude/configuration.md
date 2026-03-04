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
| Config needed outside controllers (main, beans) | `ConfigProperties::from_config()` or `with_typed()` |
| Config section as injectable bean (one-liner) | `AppBuilder::with_config_section::<T>(path)` |
| Serde-based sub-tree extraction (no ConfigProperties) | `config.get_section::<V>(path)` |

### Outside controllers: manual usage

In `main()`, services, or tests — use `ConfigProperties::from_config()` directly:
```rust
let config = R2eConfig::load("dev")?;
let db = DatabaseConfig::from_config(&config, Some("app.database"))?;
println!("url = {}", db.url);
```

Or upgrade the entire config to typed with `with_typed()` for `Deref` access:
```rust
let config = R2eConfig::load("dev")?
    .with_typed::<AppConfig>(Some("app"))?;

// Typed access via Deref
println!("{}", config.name);

// Raw access still works
let raw: String = config.get("app.name")?;
```

### Providing config to AppBuilder

There are **two ways** to provide configuration to the builder (both are pre-state methods):

#### `with_config(config)` — provide a pre-loaded config

Use when you already have an `R2eConfig` instance (hot-reload, tests, custom loading):

```rust
let config = R2eConfig::load("dev").unwrap_or_else(|_| R2eConfig::empty());
AppBuilder::new()
    .with_config(config)
    .build_state::<Services, _, _>().await
```

Also works with typed configs:
```rust
let config = R2eConfig::load("dev")?.with_typed::<AppConfig>(None)?;
AppBuilder::new()
    .with_config(config)  // provides both R2eConfig and R2eConfig<AppConfig>
```

#### `load_config::<C>(profile)` — load + type + provide in one call

Use for the common case where you just want to load from YAML files:

```rust
// Raw config only:
AppBuilder::new()
    .load_config::<()>("dev")

// With typed config struct:
AppBuilder::new()
    .load_config::<AppConfig>("dev")
```

`C` must implement `LoadableConfig` — satisfied by `()` (raw only) and any `T: ConfigProperties` (raw + typed). Panics if loading or typed construction fails.

Both methods do the same thing under the hood:
1. Store the raw config in the builder (for `serve_auto`, `with_config_section`, etc.)
2. Provide `R2eConfig` in the bean registry (injectable by beans via `#[config("key")]`)
3. If `C` is not `()`, also provide `R2eConfig<C>` in the bean registry

**Important:** `with_config` / `load_config` are **pre-state** methods — call them before `.build_state()` or `.with_state()`.

### `with_config_section()` — config section as injectable bean

Deserializes a config sub-tree and registers it as a bean in one call:

```rust
AppBuilder::new()
    .with_config(config)  // or .load_config::<()>("dev")
    .with_config_section::<CveMatchingConfig>("cve.matching")
    // CveMatchingConfig is now injectable by other beans
```

Signature: `fn with_config_section<B: DeserializeOwned + Clone + Send + Sync + 'static>(self, path: &str) -> AppBuilder<...>`.

Uses `config.get_section(path)` internally. Panics if config is not yet provided or deserialization fails. The resulting `B` is `provide()`d into the bean registry.

**`get_section()` vs `ConfigProperties::from_config()`:**
- `get_section::<V>(path)` — serde-based, works with any `#[derive(Deserialize)]` struct. No field-level `#[config(...)]` attributes.
- `ConfigProperties::from_config(&config, prefix)` — macro-based, supports `#[config(default, key, env, section)]` field attributes and validation metadata.

---

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
- `get_section::<V: DeserializeOwned>("path")` → `Result<V, ConfigError>` — collects all keys under `path.*`, rebuilds a nested JSON object, and deserializes via serde. Use for extracting an entire sub-tree into a struct without `ConfigProperties`.
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

- `#[config("key")] param: T` on `#[bean]` / `#[producer]` constructor parameters — single value only.

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
