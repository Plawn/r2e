# Configuration

R2E uses YAML-based configuration with environment variable overlay, secret resolution, and strongly-typed config sections.

## Quick start

Create `application.yaml` at your project root and wire it into `AppBuilder`:

```yaml
app:
  name: "my-app"
  greeting: "Hello"

server:
  port: 8080
```

```rust
use r2e::prelude::*;

#[derive(ConfigProperties, Clone, Debug)]
pub struct AppConfig {
    pub name: String,
    #[config(default = "Hello!")]
    pub greeting: String,
}

#[tokio::main]
async fn main() {
    AppBuilder::new()
        .load_config::<AppConfig>()    // loads YAML + env, provides AppConfig as a bean
        // ... register controllers, build state ...
        .serve_auto()
        .await
        .unwrap();
}
```

That's it — your config is loaded and available for injection everywhere.

## Configuration files & resolution order

Values are resolved in order of increasing priority — later sources override earlier ones:

1. **`application.yaml`** — base configuration, hierarchies flattened to dot-separated keys (`app.database.url`)
2. **`.env` file** — loaded into process environment via dotenvy (does **not** overwrite already-set env vars)
3. **`${...}` placeholders** — resolved in string values (see [Secrets](#secrets))
4. **Environment variables** — highest priority

### Loading

```rust
R2eConfig::load()                          // application.yaml + .env + env vars
R2eConfig::load_with_resolver(&resolver)   // custom SecretResolver
R2eConfig::from_yaml_str(yaml)             // from a YAML string (testing)
R2eConfig::empty()                         // empty config (testing)
```

## Environment variable overlay

Environment variables override any YAML key. Convention: dots become underscores, all uppercase.

| YAML key | Environment variable |
|---|---|
| `app.name` | `APP_NAME` |
| `database.url` | `DATABASE_URL` |
| `app.max-retries` | `APP_MAX_RETRIES` |
| `server.port` | `SERVER_PORT` |

## Secrets

String values in YAML can contain `${...}` placeholders resolved before the env var overlay:

```yaml
database:
  url: "${DATABASE_URL}"              # env var (default)
  password: "${env:DB_PASSWORD}"      # explicit env var
  api_key: "${file:/run/secrets/key}" # read from file (trimmed)
  kind: "${DB_KIND:postgres}"         # env var with default value
```

| Syntax | Resolution |
|---|---|
| `${VAR}` | `std::env::var("VAR")` |
| `${VAR:default}` | Env var, falls back to `default` if unset |
| `${env:VAR}` | Explicit env var lookup |
| `${env:VAR:default}` | Explicit env var with fallback |
| `${file:/path}` | Read file contents, trimmed |

### Custom secret resolver

Implement `SecretResolver` to add custom backends (e.g., Vault, AWS Secrets Manager):

```rust
use r2e::prelude::*;

struct VaultResolver { /* ... */ }

impl SecretResolver for VaultResolver {
    fn resolve(&self, reference: &str) -> Result<String, ConfigError> {
        if let Some(path) = reference.strip_prefix("vault:") {
            // fetch from Vault...
            Ok(secret)
        } else {
            DefaultSecretResolver.resolve(reference)
        }
    }
}

let config = R2eConfig::load_with_resolver(&VaultResolver { /* ... */ }).unwrap();
```

## Reading values

### `#[config("key")]` — single value injection

Inject individual values by their full dot-separated key. The field type must implement `FromConfigValue`.

```rust
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[config("app.greeting")] greeting: String,
    #[config("app.max-retries")] max_retries: i64,
    #[config("feature.enabled")] enabled: bool,
    #[config("optional.key")] maybe: Option<String>,  // None if missing
}
```

Missing required keys (non-`Option`) cause a panic at startup with a descriptive message including the expected env var name.

### Programmatic access

```rust
let config = R2eConfig::load().unwrap();

// Typed retrieval
let name: String = config.get("app.name").unwrap();
let retries: i64 = config.get("app.max-retries").unwrap();

// With default
let port: i64 = config.get_or("server.port", 3000);

// Check existence
if config.contains_key("feature.flag") {
    // ...
}

// Manual set
let mut config = config;
config.set("app.key", ConfigValue::String("value".into()));
```

### Supported types

The `FromConfigValue` trait converts raw `ConfigValue` to Rust types:

| Rust type | Accepted variants | Notes |
|---|---|---|
| `String` | String, Integer, Float, Bool | Converts via `to_string()` |
| `i64` | Integer, String (parsable) | |
| `i8`, `i16`, `i32` | Integer, String | Range-checked via `i64` |
| `u8`, `u16`, `u32`, `u64`, `usize` | Integer, String | Range-checked via `i64` |
| `f64` | Float, Integer, String | |
| `f32` | Float, Integer, String | Via `f64` cast |
| `bool` | Bool, String (`"true"/"false"/"1"/"0"/"yes"/"no"`) | Case-insensitive |
| `Option<T>` | Null → `None`, other → `Some(T)` | |
| `Vec<T>` | List → mapped items, single value → `vec![T]` | |
| `HashMap<String, V>` | Map → mapped entries | |
| Custom enum/struct | Via `#[derive(FromConfigValue)]` | Requires `serde::Deserialize` |

### Custom types with `#[derive(FromConfigValue)]`

For enums and other types, derive both `serde::Deserialize` and `FromConfigValue`:

```rust
#[derive(serde::Deserialize, FromConfigValue, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum AppMode {
    Development,
    Production,
    Staging,
}
```

```yaml
app:
  mode: "production"
```

```rust
let mode: AppMode = config.get("app.mode").unwrap();
// mode == AppMode::Production
```

Use as a field in `ConfigProperties` structs with `Option<T>`:

```rust
#[derive(ConfigProperties, Clone, Debug)]
pub struct AppConfig {
    pub name: String,
    pub mode: Option<AppMode>,  // None if absent, deserialized via serde if present
}
```

The derive works with any `Deserialize` type — enums, structs, or newtypes.

### Lists and maps

YAML sequences and maps are fully supported:

```yaml
app:
  allowed-origins:
    - "http://localhost:3000"
    - "https://prod.example.com"
  feature-flags:
    dark-mode: true
    beta: false
```

Sequences are stored both as a `List` at the parent key and individually indexed (`app.allowed-origins.0`, `app.allowed-origins.1`):

```rust
let origins: Vec<String> = config.get("app.allowed-origins").unwrap();
let first: String = config.get("app.allowed-origins.0").unwrap();
```

## Typed sections with `ConfigProperties`

For groups of related settings, define a typed struct with `#[derive(ConfigProperties)]`. The prefix is provided at the injection site or call site — there is no struct-level prefix attribute.

### Basic usage

```rust
#[derive(ConfigProperties, Clone, Debug)]
pub struct DatabaseConfig {
    /// Database connection URL
    pub url: String,

    /// Connection pool size
    #[config(default = 10)]
    pub pool_size: i64,

    /// Optional connection timeout in seconds
    pub timeout: Option<i64>,
}
```

```yaml
app:
  database:
    url: "postgres://localhost/mydb"
```

```rust
let db = DatabaseConfig::from_config(&config, Some("app.database"))?;
// url = "postgres://localhost/mydb", pool_size = 10 (default), timeout = None
```

### Field attributes

| Attribute | Effect | Example |
|---|---|---|
| *(none)* | Required. Error if missing. | `pub url: String` |
| `Option<T>` | Optional. `None` if missing. | `pub timeout: Option<i64>` |
| `#[config(default = value)]` | Fallback if missing | `#[config(default = 10)] pub pool_size: i64` |
| `#[config(default = "str")]` | String default (auto `.into()`) | `#[config(default = "hello")] pub greeting: String` |
| `#[config(key = "custom.path")]` | Override key path (relative to prefix) | `#[config(key = "jwks.url")] pub jwks_url: String` |
| `#[config(env = "VAR")]` | Env var fallback if key missing | `#[config(env = "DATABASE_URL")] pub url: String` |
| `#[config(section)]` | Nested `ConfigProperties` sub-struct | `#[config(section)] pub database: DatabaseConfig` |
| `/// doc comment` | Description in validation errors | `/// Connection timeout` |

Attributes combine: `#[config(key = "client.id", default = "my-app")]`.

Priority for a field: **YAML > env var (`#[config(env)]`) > default > error/None**.

### Custom key mapping

When the YAML hierarchy doesn't match the Rust field name:

```rust
#[derive(ConfigProperties, Clone, Debug)]
pub struct OidcConfig {
    pub issuer: Option<String>,

    #[config(key = "jwks.url")]
    pub jwks_url: Option<String>,          // reads "<prefix>.jwks.url"

    #[config(key = "client.id", default = "my-app")]
    pub client_id: String,                  // reads "<prefix>.client.id"
}
```

### Nested sections

Compose config structs hierarchically with `#[config(section)]`:

```rust
#[derive(ConfigProperties, Clone, Debug)]
pub struct DatabaseConfig {
    pub url: String,
    #[config(default = 10)]
    pub pool_size: i64,
}

#[derive(ConfigProperties, Clone, Debug)]
pub struct AppConfig {
    pub name: String,

    #[config(section)]
    pub database: DatabaseConfig,       // reads "<prefix>.database.*"

    #[config(section)]
    pub tls: Option<TlsConfig>,        // optional — None if section absent
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

`#[config(section)]` is required because the derive macro operates on tokens only — it cannot tell whether a field implements `FromConfigValue` (scalar) or `ConfigProperties` (nested struct). The attribute tells the macro to generate `T::from_config(...)` instead of `config.get::<T>(...)`.

When using `load_config::<Root>()`, all `#[config(section)]` children are **automatically registered as beans** — available for `#[inject]` in controllers and as dependencies in `#[bean]` constructors. This happens recursively, so grandchild sections are also registered.

### Doc comments as descriptions

Doc comments on fields become property descriptions, used in validation error messages:

```rust
#[derive(ConfigProperties, Clone, Debug)]
pub struct AppConfig {
    /// The display name of the application
    pub name: String,
}
```

If `app.name` is missing, the error includes: `-- The display name of the application`.

## Injection

### How to choose

| Situation | Use |
|---|---|
| 1–2 isolated values from different sections | `#[config("full.key")]` |
| Typed config struct (registered via `load_config`) | `#[inject]` on the config type |
| Config needed outside DI (main, tests) | `ConfigProperties::from_config()` |

### In controllers

With `load_config::<RootConfig>()`, config types are auto-registered as beans. Inject them with `#[inject]`:

```rust
#[derive(Controller)]
#[controller(state = Services)]
pub struct ConfigController {
    #[inject]
    root_config: RootConfig,          // auto-registered by load_config

    #[config("app.name")]
    name: String,                     // single scalar value
}

#[routes]
impl ConfigController {
    #[get("/config")]
    async fn config_info(&self) -> Json<serde_json::Value> {
        let app = &self.root_config.app;
        Json(serde_json::json!({
            "app_name": app.name,
            "greeting": app.greeting,
        }))
    }
}
```

### In beans

Nested config sections are available as bean dependencies:

```rust
#[bean]
impl NotificationService {
    pub fn new(
        bus: LocalEventBus,
        #[config("notification.capacity")] capacity: i64,
        matching: MatchingConfig,           // auto-registered config child from BeanContext
    ) -> Self {
        Self { bus, capacity: capacity as usize, matching }
    }
}
```

### In producers

```rust
#[producer]
async fn create_pool(#[config("database.url")] url: String) -> SqlitePool {
    SqlitePool::connect(&url).await.unwrap()
}
```

## AppBuilder integration

Two pre-state methods to provide config — call before `.build_state()` or `.with_state()`.

### `load_config::<C>()` (recommended)

The idiomatic way to set up configuration. Loads YAML + env, stores the raw config, provides `R2eConfig` in the bean registry, and when `C` is not `()`:
- Constructs the typed config via `C::from_config()`
- **Auto-registers all `#[config(section)]` children as beans** (recursively)
- Adds both `C` and `R2eConfig` to the compile-time type list

```rust
AppBuilder::new().load_config::<()>()           // raw config only
AppBuilder::new().load_config::<RootConfig>()   // raw + typed + children (preferred)
```

`C` must implement `LoadableConfig` — satisfied by `()` (raw only) and any `T: ConfigProperties`.

Example with nested sections:

```rust
#[derive(ConfigProperties, Clone, Debug)]
pub struct RootConfig {
    #[config(section)]
    pub app: AppConfig,           // auto-registered as a bean
    #[config(section)]
    pub database: DatabaseConfig, // auto-registered as a bean
}

AppBuilder::new()
    .load_config::<RootConfig>()  // RootConfig, AppConfig, DatabaseConfig all available
    // ...
```

### `with_config(config)` (pre-loaded)

Only needed when you have a pre-loaded `R2eConfig` (tests, hot-reload, custom loading). Prefer `load_config` in all other cases.

```rust
let config = R2eConfig::load().unwrap_or_else(|_| R2eConfig::empty());
AppBuilder::new().with_config(config)
```

Both methods store the raw config in the builder and provide `R2eConfig` in the bean registry.

Your state type must implement `FromRef` for `R2eConfig`:

```rust
impl axum::extract::FromRef<AppState> for R2eConfig {
    fn from_ref(state: &AppState) -> Self {
        state.config.clone()
    }
}
```

## Startup validation

R2E validates configuration at startup. When a controller is registered, all `#[config("key")]` fields are checked. Missing required keys cause a panic with a clear error:

```
=== CONFIGURATION ERRORS (controller: MyController) ===

Missing configuration keys:
  - `MyController`: key 'app.database.url' (String) — set env var `APP_DATABASE_URL` -- Database connection URL
  - `MyController`: key 'app.secret' (String) — set env var `APP_SECRET`
============================
```

For `ConfigProperties` sections, validation also catches type mismatches and garde constraint violations.

## `serve_auto()`

Instead of hardcoding the listen address, read it from configuration:

```yaml
server:
  host: "0.0.0.0"
  port: 8080
```

```rust
AppBuilder::new()
    // ... build state, register controllers ...
    .serve_auto()  // reads server.host and server.port from config
    .await
    .unwrap();
```

| Config key | Type | Default |
|---|---|---|
| `server.host` | String | `"0.0.0.0"` |
| `server.port` | u16 | `3000` |

If keys are missing, defaults are used. This replaces `.serve("0.0.0.0:3000")` for production setups where the address should be configurable per environment.

## Testing

```rust
// Empty config
let config = R2eConfig::empty();

// From YAML string
let config = R2eConfig::from_yaml_str(r#"
app:
  name: "test-app"
  greeting: "hi"
"#).unwrap();

// Programmatic setup
let mut config = R2eConfig::empty();
config.set("app.name", ConfigValue::String("Test App".into()));
config.set("app.port", ConfigValue::Integer(8080));
```
