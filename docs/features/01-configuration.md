# Feature 1 — Configuration

## TL;DR

Typed configuration from YAML + environment, with `${...}` secret resolution and strongly-typed sections. `R2eConfig` stores dot-separated keys (`app.database.url`); resolution order is `application.yaml` → profile file → `.env` → `${...}` placeholders → config providers → `R2E_`-prefixed env vars (`R2E_SERVER_PORT` overrides `server.port`; strict `_`→`.` mapping, so keys with `-` or an in-segment `_` are set through a `${VAR}` placeholder instead). Load with `R2eConfig::load()` (never fails — env is always overlaid) or drive it from the builder via `.load_config::<RootConfig>()`. Inject values into controllers with `#[config("app.greeting")]`. Deserialize a whole section with serde through `SerdeConfig<T>`; plugins merge builder > file > preset through `Layered<C>`. Custom base file via `.with_config_file("patina.yaml")` (profile overlay: `patina-test.yaml`).


## Goal

Provide a typed configuration system loaded from YAML files and environment variables, with secret resolution and strongly-typed config sections.

## Key concepts

### R2eConfig

`R2eConfig` is the central configuration container. It stores values as dot-separated keys (e.g., `app.database.url`).

### Resolution order

1. `application.yaml` — base configuration
2. `application-<profile>.yaml` — profile overlay (the file name derives from the base: `<stem>-<profile>.<ext>`)
3. `.env` file — loaded into process environment (does not overwrite existing env vars)
4. `${...}` placeholders — resolved in string values (see [Secrets and placeholders](#secrets-and-placeholders))
5. Config providers registered with `.with_config_provider(..)`, then placeholders again for the strings they supplied
6. `R2E_`-prefixed environment variables — final override (`R2E_SERVER_PORT` → `server.port`, see [Environment variables](#environment-variables))

The active profile is: the explicit argument > `R2E_PROFILE` > the `r2e.profile` key > `"default"`.

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
use r2e::config::{R2eConfig, ConfigValue};

let config = R2eConfig::load().unwrap_or_else(|_| R2eConfig::empty());
```

`load()` succeeds even if the YAML file is absent (environment variables are always overlaid). To ensure required keys are present, check and set defaults:

```rust
let mut config = R2eConfig::load().unwrap_or_else(|_| R2eConfig::empty());

if config.get::<String>("app.name").is_err() {
    config.set("app.name", ConfigValue::String("Default App".into()));
}
```

#### Custom base file

To use a domain-specific file name instead of `application.yaml`:

```rust
let config = R2eConfig::load_from("patina.yaml")?;

// Or on the builder, before load_config:
AppBuilder::new()
    .with_config_file("patina.yaml")
    .load_config::<RootConfig>()
```

The profile overlay file derives from the base name (`patina.yaml` + profile
`test` → `patina-test.yaml`), and secrets / env overlay apply as usual. Unlike
`load()`, an explicitly requested file that does not exist is an error — a
typo'd name would otherwise silently yield an empty config.

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
| Custom enum/struct | Via `#[derive(FromConfigValue)]` | Requires `serde::Deserialize` |

### Custom types via `#[derive(FromConfigValue)]`

For enums and other types, derive `FromConfigValue` alongside `serde::Deserialize`:

```rust
#[derive(serde::Deserialize, FromConfigValue, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum AppMode {
    Development,
    Production,
    Staging,
}
```

### 4. Injection in a controller via `#[config]`

The `#[config("key")]` field attribute on a controller automatically injects the value from configuration at request time:

```rust
use r2e::prelude::*;

#[controller(path = "/")]
pub struct MyController {
    #[config("app.greeting")]
    greeting: String,

    #[config("app.name")]
    app_name: String,
}

#[routes]
impl MyController {
    #[get("/greeting")]
    async fn greeting(&self) -> r2e::http::Json<serde_json::Value> {
        r2e::http::Json(serde_json::json!({
            "greeting": self.greeting,
            "app": self.app_name,
        }))
    }
}
```

### 5. Typed config sections

For groups of related settings, define typed config structs. Use `load_config::<RootConfig>()` to auto-register all nested sections as beans:

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

#[derive(ConfigProperties, Clone, Debug)]
pub struct RootConfig {
    #[config(section)]
    pub app: AppConfig,
}

#[controller(path = "/config")]
pub struct ConfigController {
    #[inject]
    root_config: RootConfig,     // auto-registered by load_config
}
```

Field attributes for `ConfigProperties`:
- `#[config(default = value)]` — fallback if key is missing
- `#[config(key = "nested.key")]` — override the config key path
- `#[config(env = "VAR")]` — explicit env var fallback
- `#[config(section)]` — nested `ConfigProperties` struct
- `Option<T>` type — makes the field optional (`None` if absent)

### Serde-deserialized sections: `SerdeConfig<T>`

When a section is already a `serde::Deserialize` type (shared with another
tool, full of enums, `#[serde(flatten)]`…), skip the derive and wrap it in
`SerdeConfig<T>` (in the prelude; `T: DeserializeOwned + Clone + Send + Sync + 'static`):

```rust,ignore
#[derive(serde::Deserialize, Clone, Debug)]
pub struct EngineConfig {
    pub engines: Vec<Engine>,
    pub limits: std::collections::HashMap<String, u32>,
}

AppBuilder::new()
    .load_config::<SerdeConfig<EngineConfig>>()   // provides SerdeConfig<EngineConfig> AND EngineConfig
    // ...

#[controller(path = "/engines")]
pub struct EngineController {
    #[inject] cfg: EngineConfig,                  // the plain type is a bean too
}
```

`SerdeConfig<T>` is a ready-made `ConfigProperties` impl. It works everywhere
a section does:
- as a `#[config(section)]` field
- as a plugin's `Config`
- with `provide_config`

`Deref` and `into_inner()` give access to `T`.

Behaviour:

- The flat keys stay the source of truth. The value is rebuilt from them,
  so the env overlay and indexed keys (`R2E_APP_ENGINES_0_KEY`) apply.
- An absent or `null` section deserializes from an empty map. A `T` whose
  fields all have serde defaults loads without any YAML.
- Scalars written as strings (from env or placeholders) are coerced to the
  field's type for typed hints. This does not happen under
  `deserialize_any` (untagged enums, `flatten`, `serde_json::Value`).
- An error is `ConfigError::Deserialize { key, message }` with the full key
  path. There are two trade-offs against the derive: only the first error is
  reported (no aggregated report), and there is no `PropertyMeta`, so
  nothing shows up in config docs.

### Reading a raw subtree: `R2eConfig::tree`

`config.tree(Some("plan.models"))` returns the subtree under a prefix as a
nested `ConfigValue` (`Map` / `List`), or `None` when nothing is under it
(`tree(None)` is the whole config). It is what `SerdeConfig` deserializes
from, and it returns the YAML as written:

- an empty table (`limits: {}`) is an empty `Map`, not a missing field;
- a mapping key keeps its dots — `gpt-4.1:` is one key, not `gpt-4` → `1`;
- a digit-only mapping key (`"0"`, `200`) stays a key instead of turning
  its parent into a list.

A dotted override (`override_config_value`, a config provider, `set`)
replaces the same-named YAML key, or lands under the longest YAML map key it
starts with: `plan.models.gpt-4.1.timeout` goes under `gpt-4.1`. Flat access
(`get("a.b")`, `sub_keys`, `has_prefix`) is unchanged. `R2E_` variables
cannot address keys containing a dot or a dash (see below).

### Prerequisite for `#[config]`

Nothing to wire by hand. `R2eConfig` is itself a bean in the resolved graph, so `#[config("key")]` fields resolve through it automatically once the app is built with `load_config::<C>()`. No `FromRef` impl is needed. `#[config(...)]` fields and typed `#[config_section]` structs are unchanged.

### 6. Registering config in AppBuilder

```rust
// Load + auto-register typed config + children — the sole config registration point
AppBuilder::new()
    .load_config::<RootConfig>()           // loads YAML + env, provides RootConfig + children
    // ...

// Tests only: stash an in-memory config for the next load_config::<C>() to use
// instead of reading disk (part of the override_bean/override_config_value family)
AppBuilder::new()
    .override_config(config)
    .load_config::<RootConfig>()
    // ...
```

> **Large apps:** each typed config section and every provided/registered value is a bean. Apps with more than ~127 beans need `#![recursion_limit = "512"]` in the binary and library crate roots (`main.rs` and `lib.rs`); `r2e doctor` warns as the bean count approaches the threshold.

## Secrets and placeholders

String values in YAML can contain `${...}` placeholders resolved before env var overlay:

```yaml
database:
  url: "${DATABASE_URL}"              # env var (default)
  password: "${env:DB_PASSWORD}"      # explicit env var
  api_key: "${file:/run/secrets/key}" # read from file (trimmed)
  kind: "${DB_KIND:postgres}"         # env var with default value
```

Default values are supported with `${VAR:default}` syntax. This also works with the explicit prefix: `${env:VAR:default}`. File references (`${file:...}`) do not support defaults.

Placeholder rules:

- **The default is everything after the first `:`, verbatim.** For example,
  `${PORT:-1}` defaults to `-1`, and a URL default keeps its own colons
  (`${DB:postgres://localhost:5432/app}`). `${VAR:}` defaults to the empty
  string.
- **There is no shell `:-` operator.** `${VAR:-fallback}` defaults to
  `-fallback`, with the dash included.
- **A default applies only when the variable is unset.** A variable set to
  the empty string stays empty.
- **Defaults nest.** `${PRIMARY:${SECONDARY:localhost}}` tries `PRIMARY`,
  then `SECONDARY`, then the literal.
- **Resolved values are rendered again.** With
  `DATABASE_URL=postgres://${DB_HOST}/app` in the environment, `${DB_HOST}`
  is resolved too.
- **Cycles are an error.** `A=${B}` with `B=${A}` fails with
  `ConfigError::Load("Placeholder cycle A -> B -> A in: …")` instead of
  looping.
- **Every missing variable is reported at once.** One
  `ConfigError::NotFound("A, B, env:C")` names every unresolved placeholder
  that has no default. It covers strings, lists and maps, so you don't have
  to fix them one restart at a time.

Custom backends can implement the `SecretResolver` trait and pass it via `R2eConfig::load_with_resolver()`.

## Environment variables

Only variables prefixed with `R2E_` are read. The prefix is stripped, the
rest is lowercased, and every `_` becomes a `.`. They override every other
source:

```
Environment variable          →  Config key
R2E_SERVER_PORT=8080          →  server.port
R2E_APP_NAME=prod             →  app.name
R2E_APP_ENGINES_0_KEY=abc     →  app.engines.0.key   (list index)
```

The mapping is **strict**, so a key containing a `-` or an `_` inside a
segment cannot be reached this way. `security.jwt.jwks-url` and
`database.pool_size` are two examples: `R2E_DATABASE_POOL_SIZE` maps to
`database.pool.size`. For such keys, put a placeholder in the YAML and set
that variable instead:

```yaml
database:
  pool_size: "${DB_POOL_SIZE:10}"
security:
  jwt:
    jwks-url: "${JWKS_URL}"
```

Unprefixed variables (`APP_NAME`, `DATABASE_URL`) are **ignored** by the
overlay. They are read only through a `${VAR}` placeholder, or by a
`ConfigProperties` field marked `#[config(env = "VAR")]`.

## Startup validation

Required config keys are checked when a controller is registered, not on the
first request.

- **`register_controller::<C>()`** checks every non-`Option` `#[config]`
  field. If any are missing it **panics**, and the panic message is a report
  listing all of them at once:

  ```text
  === CONFIGURATION ERRORS (controller: MyController) ===
  ...
  ============================
  ```

  Each missing key comes with a hint. For a purely dotted key (`app.name`)
  the hint is the `R2E_` variable to set. For a key the overlay cannot
  reach (one with a `-` or an in-segment `_`), it tells you to set the key
  in `application.yaml` or through a `${VAR}` placeholder.
- **`try_register_controller::<C>()`** runs the same check but returns
  `Result<_, ConfigValidationError>` instead of panicking.
  `ConfigValidationError.errors` holds one `MissingKeyError` per key.
  Services registered with `spawn_service` are checked the same way.
- **`load_config::<C>()`** validates the whole typed section tree up front.
  The errors are parked until build time:
  - `try_build_state()` returns them;
  - `build_state()` panics with the same report, which lists every missing
    key at once.

  A controller registered inside a `#[module]` reports
  `BeanError::ControllerConfig`.

An `Option<T>` field, or a field with `#[config(default = ..)]`, is never
reported missing.

## Layered plugin configuration

Configurable plugins (`Cors`, `HttpTrace`, `Prometheus`, `MetricsFacade`,
`McpServer`) resolve every setting through the same precedence chain:

```text
explicit builder setter  >  file section  >  preset  >  C::default()
```

The mechanism is `r2e::config::Layered<C>`. It holds the code tiers (the
explicit setters and the preset) and resolves them against the section
loaded from the file at build time:

```rust,ignore
use r2e::config::Layered;
use r2e::prelude::*;

#[derive(ConfigProperties, Clone, Debug, Default)]
pub struct MyConfig {
    pub timeout_secs: Option<u64>,
    pub endpoints: Option<Vec<String>>,
}

pub struct MyPlugin { cfg: Layered<MyConfig> }

impl MyPlugin {
    pub fn new() -> Self { Self { cfg: Layered::new() } }
    /// A shared baseline — the app's YAML overrides it key by key.
    pub fn preset(cfg: MyConfig) -> Self { Self { cfg: Layered::with_preset(cfg) } }
    /// An explicit setting — wins over the file.
    pub fn timeout_secs(mut self, s: u64) -> Self {
        self.cfg.set(|c| c.timeout_secs = Some(s));
        self
    }
}

// in Plugin::build(self, _, config: Option<MyConfig>, _):
let cfg = self.cfg.resolve(config);
let timeout = cfg.timeout_secs.unwrap_or(30);
```

`Layered` API:
- constructors: `new()`, `with_preset(c)`, `with_explicit(c)`
- edit a tier: `set(|c| ..)` (explicit tier), `set_preset(|c| ..)`, `replace_preset(c)`
- read a tier: `explicit()`, `preset()`
- `resolve(file: Option<C>) -> C`

The merge is `ConfigProperties::overlay(self, under)`, which
`#[derive(ConfigProperties)]` generates field by field:

| Field kind | Overlay result |
|---|---|
| `Option<T>` | the first tier that set it (a list is **replaced**, never concatenated) |
| required `#[config(section)]` | merged recursively |
| optional `#[config(section)]` | merged if the section is layerable, otherwise taken whole |
| any other field | `self`'s value |

`Layered<C>` requires `C::LAYERABLE`: every field must be an `Option<T>` or
a layerable `#[config(section)]`, so that "not set" can be told apart from
a value. Using `Layered` with a non-layerable `C` fails a post-monomorphization
const assertion. It fires on `cargo build` / `cargo test`, **not** on
`cargo check`. A hand-written `ConfigProperties` impl defaults to
`LAYERABLE = false`, and its `overlay` returns `self`.

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
"#).unwrap();
```

## Validation criteria

```bash
curl -H "Authorization: Bearer <token>" http://localhost:3000/greeting
# → {"greeting":"Welcome!"}

curl http://localhost:3000/config
# → {"app_name":"My Application","app_version":"0.1.0"}
```
