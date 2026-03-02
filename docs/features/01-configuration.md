# Feature 1 — Configuration

## Goal

Provide a typed configuration system loaded from YAML files and environment variables, with profile support (`dev`, `prod`, `test`), secret resolution, and strongly-typed config sections.

## Key concepts

### R2eConfig

`R2eConfig` is the central configuration container. It stores values as dot-separated keys (e.g., `app.database.url`). It supports an optional typed layer via `R2eConfig<T>` where `T: ConfigProperties`.

### Resolution order

1. `application.yaml` — base configuration
2. `application-{profile}.yaml` — profile override
3. `.env` file — loaded into process environment (does not overwrite existing env vars)
4. `.env.{profile}` file — profile-specific env file
5. `${...}` secret placeholders — resolved in string values
6. Environment variables — final override (convention: `APP_DATABASE_URL` ↔ `app.database.url`)

### Active profile

Determined by: `R2E_PROFILE` env var > `load()` argument > default `"dev"`.

## Usage

### 1. Configuration file

Create an `application.yaml` file at the workspace root:

```yaml
app:
  name: "My Application"
  greeting: "Welcome!"
  version: "0.1.0"

database:
  url: "sqlite::memory:"
  pool_size: 10
```

### 2. Loading configuration

```rust
use r2e_core::config::{R2eConfig, ConfigValue};

let config = R2eConfig::load("dev").unwrap_or_else(|_| R2eConfig::empty());
```

`load()` succeeds even if the YAML file is absent (environment variables are always overlaid). To ensure required keys are present, check and set defaults:

```rust
let mut config = R2eConfig::load("dev").unwrap_or_else(|_| R2eConfig::empty());

if config.get::<String>("app.name").is_err() {
    config.set("app.name", ConfigValue::String("Default App".into()));
}
```

### 3. Reading values

```rust
// Typed retrieval
let name: String = config.get("app.name").unwrap();
let pool_size: i64 = config.get("database.pool_size").unwrap();
let debug: bool = config.get("app.debug").unwrap_or(false);

// With default value
let timeout: i64 = config.get_or("app.timeout", 30);
```

### Supported types

| Type | ConfigValue | Conversion |
|------|------------|------------|
| `String` | All types | `.to_string()` |
| `i64` | `Integer`, `String` (parsable) | Direct or parse |
| `f64` | `Float`, `Integer`, `String` | Direct or parse |
| `bool` | `Bool`, `String` (`"true"/"false"/"1"/"0"/"yes"/"no"`) | Direct or parse |
| `Option<T>` | `Null` → `None`, other → `Some(T)` | Recursive |
| `Vec<T>` | `List` → mapped items, single value → `vec![T]` | Recursive |
| `HashMap<String, V>` | `Map` → mapped entries | Recursive |

### 4. Injection in a controller via `#[config]`

The `#[config("key")]` field attribute on a controller automatically injects the value from configuration at request time:

```rust
use r2e_core::prelude::*;

#[derive(Controller)]
#[controller(state = Services)]
pub struct MyController {
    #[config("app.greeting")]
    greeting: String,

    #[config("app.name")]
    app_name: String,
}

#[routes]
impl MyController {
    #[get("/greeting")]
    async fn greeting(&self) -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({
            "greeting": self.greeting,
            "app": self.app_name,
        }))
    }
}
```

### 5. Typed config sections with `#[config_section]`

For groups of related settings, define a typed config struct and inject it as a whole:

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
```

Field attributes for `ConfigProperties`:
- `#[config(default = value)]` — fallback if key is missing
- `#[config(key = "nested.key")]` — override the config key path
- `#[config(env = "VAR")]` — explicit env var fallback
- `#[config(section)]` — nested `ConfigProperties` struct
- `Option<T>` type — makes the field optional (`None` if absent)

### Prerequisite for `#[config]`

The state type (`Services`) must implement `FromRef<Services>` for `R2eConfig`:

```rust
impl axum::extract::FromRef<Services> for R2eConfig {
    fn from_ref(state: &Services) -> Self {
        state.config.clone()
    }
}
```

### 6. Registering config in AppBuilder

```rust
AppBuilder::new()
    .with_state(services)
    .with_config(config)
    // ...
```

## Secrets

String values in YAML can contain `${...}` placeholders resolved before env var overlay:

```yaml
database:
  url: "${DATABASE_URL}"              # env var (default)
  password: "${env:DB_PASSWORD}"      # explicit env var
  api_key: "${file:/run/secrets/key}" # read from file (trimmed)
```

Custom backends can implement the `SecretResolver` trait and pass it via `R2eConfig::load_with_resolver()`.

## Environment variables

Environment variables override any YAML value. The naming convention is:

```
YAML key          →  Environment variable
app.database.url  →  APP_DATABASE_URL
app.name          →  APP_NAME
```

Conversion: lowercase, replace `_` with `.`.

## Startup validation

When a controller is registered with `AppBuilder`, all `#[config]` and `#[config_section]` fields are validated. Missing required keys cause a panic with a clear error message including the expected env var name.

## Testing

In tests, use `R2eConfig::empty()` to create an empty configuration and set values programmatically:

```rust
let mut config = R2eConfig::empty();
config.set("app.name", ConfigValue::String("Test App".into()));
config.set("app.greeting", ConfigValue::String("Hello from tests!".into()));
```

Or parse a YAML string:

```rust
let config = R2eConfig::from_yaml_str(r#"
app:
  name: "test-app"
"#, "test").unwrap();
```

## Validation criteria

```bash
curl -H "Authorization: Bearer <token>" http://localhost:3000/greeting
# → {"greeting":"Welcome!"}

curl http://localhost:3000/config
# → {"app_name":"My Application","app_version":"0.1.0"}
```
