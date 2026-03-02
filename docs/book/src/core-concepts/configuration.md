# Configuration

R2E uses YAML-based configuration with profile support, environment variable overlay, secret resolution, and strongly-typed config sections.

## Configuration files

Create `application.yaml` in your project root:

```yaml
app:
  name: "my-app"
  greeting: "Hello"
  max-retries: 3

database:
  url: "sqlite:data.db"
  pool_size: 10

server:
  port: 8080
```

## Resolution order

Configuration values are resolved in order of increasing priority — later sources override earlier ones:

1. `application.yaml` — base configuration
2. `application-{profile}.yaml` — profile-specific overrides
3. `.env` file — loaded into process environment (does **not** overwrite already-set env vars)
4. `.env.{profile}` file — profile-specific env file
5. `${...}` secret placeholders — resolved in string values (see [Secrets](#secrets))
6. Environment variables — highest priority (e.g., `APP_DATABASE_URL` overrides `app.database.url`)

### Active profile

The active profile is determined by (in order of priority):

1. `R2E_PROFILE` environment variable
2. Argument passed to `R2eConfig::load("dev")`
3. Default: `"dev"`

### Profile overrides

Create profile-specific files that merge on top of the base config:

```yaml
# application-dev.yaml
database:
  url: "sqlite::memory:"

# application-prod.yaml
database:
  url: "${DATABASE_URL}"
server:
  port: 80
```

## Loading configuration

```rust
// Load base + profile overrides + env vars
let config = R2eConfig::load("dev").unwrap();

// R2E_PROFILE=prod overrides the "dev" argument
let config = R2eConfig::load("dev").unwrap();

// From a YAML string (useful in tests)
let config = R2eConfig::from_yaml_str(r#"
app:
  name: "test-app"
"#, "test").unwrap();

// Empty config (useful in tests)
let config = R2eConfig::empty();
```

## Environment variable overlay

Environment variables override any YAML key. The convention is: dots become underscores, all uppercase.

| YAML key | Environment variable |
|----------|---------------------|
| `app.name` | `APP_NAME` |
| `database.url` | `DATABASE_URL` |
| `app.max-retries` | `APP_MAX_RETRIES` |
| `server.port` | `SERVER_PORT` |

## Secrets

String values in YAML can contain `${...}` placeholders that are resolved before env var overlay. Three resolution strategies are supported:

```yaml
database:
  url: "${DATABASE_URL}"              # env var (default)
  password: "${env:DB_PASSWORD}"      # explicit env var
  api_key: "${file:/run/secrets/key}" # read from file (trimmed)
```

| Syntax | Resolution |
|--------|-----------|
| `${VAR}` | `std::env::var("VAR")` |
| `${env:VAR}` | Explicit env var lookup |
| `${file:/path}` | Read file contents, trimmed |

### Custom secret resolver

Implement the `SecretResolver` trait to add custom backends (e.g., Vault, AWS Secrets Manager):

```rust
use r2e::prelude::*;

struct VaultResolver { /* ... */ }

impl SecretResolver for VaultResolver {
    fn resolve(&self, reference: &str) -> Result<String, ConfigError> {
        if let Some(path) = reference.strip_prefix("vault:") {
            // fetch from Vault...
            Ok(secret)
        } else {
            // fall back to default behavior
            DefaultSecretResolver.resolve(reference)
        }
    }
}

let config = R2eConfig::load_with_resolver("prod", &VaultResolver { /* ... */ }).unwrap();
```

## Using configuration in controllers

### Raw key injection with `#[config("key")]`

Inject individual values by key. The type must implement `FromConfigValue`.

```rust
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[config("app.greeting")] greeting: String,
    #[config("app.max-retries")] max_retries: i64,
    #[config("feature.enabled")] enabled: bool,
    #[config("optional.key")] maybe: Option<String>,
}
```

Values are resolved at request time from the `R2eConfig` stored in app state. Missing required keys (non-`Option`) panic with a descriptive message that includes the expected env var name.

### Typed sections with `#[config_section]`

For groups of related settings, use `#[derive(ConfigProperties)]` to define a typed config struct, then inject it with `#[config_section]`:

```rust
#[derive(ConfigProperties, Clone, Debug)]
#[config(prefix = "app")]
pub struct AppConfig {
    /// Application name
    pub name: String,

    /// Welcome greeting
    #[config(default = "Hello!")]
    pub greeting: String,

    /// Application version
    pub version: Option<String>,
}

#[derive(Controller)]
#[controller(state = Services)]
pub struct ConfigController {
    #[config_section]
    app_config: AppConfig,
}

#[routes]
impl ConfigController {
    #[get("/config")]
    async fn config_info(&self) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "app_name": self.app_config.name,
            "greeting": self.app_config.greeting,
        }))
    }
}
```

## Typed configuration with `ConfigProperties`

The `#[derive(ConfigProperties)]` macro generates a strongly-typed configuration section from a struct. It maps YAML keys to struct fields, supports defaults, optional fields, custom key mapping, env var overrides, and nested sections.

### Basic usage

```rust
#[derive(ConfigProperties, Clone, Debug)]
#[config(prefix = "app.database")]
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

With this YAML:

```yaml
app:
  database:
    url: "postgres://localhost/mydb"
```

- `url` is required — missing it produces a startup error
- `pool_size` defaults to `10` if not provided
- `timeout` is `None` if absent

### Field attributes

| Attribute | Description | Example |
|-----------|------------|---------|
| `#[config(default = value)]` | Default if key is missing | `#[config(default = 10)]` |
| `#[config(key = "nested.key")]` | Override the config key path | `#[config(key = "jwks.url")]` |
| `#[config(env = "VAR")]` | Explicit env var fallback | `#[config(env = "API_KEY")]` |
| `#[config(section)]` | Nested `ConfigProperties` | `#[config(section)]` |
| `Option<T>` type | Makes field optional | `pub timeout: Option<i64>` |

### Custom key mapping

When the YAML hierarchy doesn't match the Rust field name:

```rust
#[derive(ConfigProperties, Clone, Debug)]
#[config(prefix = "oidc")]
pub struct OidcConfig {
    pub issuer: Option<String>,

    #[config(key = "jwks.url")]
    pub jwks_url: Option<String>,          // reads from "oidc.jwks.url"

    #[config(key = "client.id", default = "my-app")]
    pub client_id: String,                  // reads from "oidc.client.id"
}
```

### Nested sections

Compose config structs hierarchically with `#[config(section)]`:

```rust
#[derive(ConfigProperties, Clone, Debug)]
#[config(prefix = "app.database")]
pub struct DatabaseConfig {
    pub url: String,
    #[config(default = 10)]
    pub pool_size: i64,
}

#[derive(ConfigProperties, Clone, Debug)]
#[config(prefix = "app")]
pub struct AppConfig {
    pub name: String,

    #[config(section)]
    pub database: DatabaseConfig,  // reads from "app.database.*"
}
```

### Doc comments as descriptions

Doc comments on fields become property descriptions, used in validation error messages and introspection:

```rust
#[derive(ConfigProperties, Clone, Debug)]
#[config(prefix = "app")]
pub struct AppConfig {
    /// The display name of the application
    pub name: String,
}
```

If `app.name` is missing, the error message includes: `-- The display name of the application`.

### Typed config on `R2eConfig`

You can upgrade an `R2eConfig` to carry a typed layer via `Deref`:

```rust
let config = R2eConfig::load("dev")?
    .with_typed::<AppConfig>()?;

// Typed field access via Deref
println!("{}", config.name);

// Raw access still works
let raw: String = config.get("app.name")?;
```

## Using configuration in beans and producers

### In beans

```rust
#[bean]
impl NotificationService {
    pub fn new(
        bus: LocalEventBus,
        #[config("notification.capacity")] capacity: i64,
    ) -> Self {
        Self { bus, capacity: capacity as usize }
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

## Programmatic access

```rust
let config = R2eConfig::load("dev").unwrap();

// Typed retrieval
let name: String = config.get("app.name").unwrap();
let retries: i64 = config.get("app.max-retries").unwrap();

// With default value
let port: i64 = config.get_or("server.port", 3000);

// Check existence
if config.contains_key("feature.flag") {
    // ...
}

// Manual set
let mut config = config;
config.set("app.key", ConfigValue::String("value".into()));

// Active profile
println!("Profile: {}", config.profile());
```

## Supported types

The `FromConfigValue` trait converts raw `ConfigValue` entries to Rust types:

| Rust type | Accepted `ConfigValue` variants | Notes |
|-----------|-------------------------------|-------|
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

Implement `FromConfigValue` for custom types if needed.

## Lists and maps in YAML

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

Sequences are stored both as a `List` value at the parent key and individually indexed (`app.allowed-origins.0`, `app.allowed-origins.1`).

```rust
// As a Vec
let origins: Vec<String> = config.get("app.allowed-origins").unwrap();

// Individual access
let first: String = config.get("app.allowed-origins.0").unwrap();
```

## Startup validation

R2E validates configuration at startup. When a controller is registered with `AppBuilder`, all `#[config("key")]` fields and `#[config_section]` fields are checked. If required keys are missing, the application panics with a clear error:

```
=== CONFIGURATION ERRORS (controller: MyController) ===

Missing configuration keys:
  - `MyController`: key 'app.database.url' (String) — set env var `APP_DATABASE_URL` -- Database connection URL
  - `MyController`: key 'app.secret' (String) — set env var `APP_SECRET`
============================
```

For `ConfigProperties` sections, validation also catches type mismatches and garde constraint violations.

## Providing config to the app

Pass the config to `AppBuilder` so controllers and extractors can access it:

```rust
let config = R2eConfig::load("dev").unwrap();

AppBuilder::new()
    .provide(config.clone())
    .build_state::<AppState, _, _>()
    .await
    .with_config(config)  // makes it available for #[config] fields
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000")
    .await;
```

Your state type must implement `FromRef` for `R2eConfig`:

```rust
impl axum::extract::FromRef<AppState> for R2eConfig {
    fn from_ref(state: &AppState) -> Self {
        state.config.clone()
    }
}
```

## Testing

```rust
// Empty config
let config = R2eConfig::empty();

// From YAML string
let config = R2eConfig::from_yaml_str(r#"
app:
  name: "test-app"
  greeting: "hi"
"#, "test").unwrap();

// Programmatic setup
let mut config = R2eConfig::empty();
config.set("app.name", ConfigValue::String("Test App".into()));
config.set("app.port", ConfigValue::Integer(8080));
```
