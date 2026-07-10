# Configuration

## Overview

R2E configuration flows through three layers:

1. **R2eConfig** — flat key-value store, loaded from YAML + env
2. **ConfigProperties** — typed structs derived from R2eConfig
3. **Injection** — `#[config("key")]` for scalars, `#[inject]` for typed config sections (via `load_config`)

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
4. Environment variables **prefixed with `R2E_`** (`R2E_SERVER_PORT=8080` ↔ `server.port = "8080"`). The `R2E_` prefix is stripped, the remainder is lowercased, and `_` becomes `.`. Unprefixed process env vars are ignored so the config namespace does not collide with the general process environment. Use `#[config(env = "VAR")]` on a `ConfigProperties` field if you need to pull in a specific unprefixed variable.

### Methods

| Method | Returns | Description |
|---|---|---|
| `get::<V>("key")` | `Result<V, ConfigError>` | Typed retrieval. `V: FromConfigValue` |
| `try_get::<V>("key")` | `Option<V>` | Returns `None` on missing key or type mismatch |
| `get_or("key", default)` | `V` | With fallback |
| `contains_key("key")` | `bool` | Key existence |
| `has_prefix("prefix")` | `bool` | Any key under the prefix (empty YAML key = absent). Section presence primitive. |
| `sub_keys("prefix")` | `Vec<String>` | Distinct immediate child segments, sorted. Map-section primitive. |
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

### Custom types via `#[derive(FromConfigValue)]`

For enums and other types that implement `serde::Deserialize`, derive `FromConfigValue` to bridge serde deserialization:

```rust
#[derive(serde::Deserialize, FromConfigValue, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum AppMode {
    Development,
    Production,
    Staging,
}
```

The derive delegates to `deserialize_value()`, which converts `ConfigValue` → `serde_json::Value` → `T`. Works with any `Deserialize` type (enums, structs, etc.). Use standard serde attributes (`#[serde(rename_all)]`, etc.) for customization.

The `deserialize_value<T: DeserializeOwned>(value, key)` helper is also available for manual `FromConfigValue` impls.

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
| `#[config(section, default)]` | Section falling back to `Default::default()` when its prefix is absent (requires `T: Default`) | `#[config(section, default)] pub server: ServerConfig` |
| `#[config(section)]` on `HashMap`/`BTreeMap<String, T>` | Map-valued section: one `T: ConfigProperties` entry per sub-key; absent prefix → empty map | `#[config(section)] pub upstreams: HashMap<String, UpstreamConfig>` |
| `#[config(skip)]` | Never read from config; initialized with `Default::default()`. Not combinable with other config attrs. | `#[config(skip)] resolved_token: Option<String>` |
| `/// doc comment` | Description in validation errors | `/// Connection timeout` |

Attributes combine: `#[config(key = "client.id", default = "my-app")]`.

Priority for a field: **YAML > env var (`#[config(env)]`) > default > error/None**.

### Optional sections are presence-based

`#[config(section)] pub tls: Option<TlsConfig>` resolves to `None` iff **no
key lives under the section prefix** (checked with `R2eConfig::has_prefix`;
an empty YAML key `tls:` counts as absent). If the prefix is present, the
section is parsed and any error surfaces — a present-but-invalid section is
an error, not a silent `None`. This makes `Option<Section>` reliable even
when every field of the section has a default.

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

### Map-valued sections

`#[config(section)]` on a `HashMap<String, T>` or `BTreeMap<String, T>` (key
must be `String`, `T: ConfigProperties`) enumerates the immediate sub-keys of
the section prefix (`R2eConfig::sub_keys`) and parses each as its own
section. An absent prefix yields an empty map. Entries are **not** registered
as beans (all entries share one type).

```rust
#[derive(ConfigProperties, Clone, Debug)]
pub struct RegistryConfig {
    #[config(section)]
    pub upstreams: HashMap<String, UpstreamConfig>,   // one entry per sub-key
}
```

```yaml
upstreams:
  npm:
    url: "https://registry.npmjs.org"
  docker:
    url: "https://registry-1.docker.io"
    enabled: false
```

### Tagged enum sections

`#[derive(ConfigProperties)]` works on enums with `#[config(tag = "...")]` —
the tag key (relative to the prefix) selects the variant; the selected
variant's payload is parsed as a section at the **same** prefix
(internally-tagged, like patina's storage backend switch):

```rust
#[derive(ConfigProperties, Clone, Debug)]
#[config(tag = "backend")]                 // reads "<prefix>.backend"
pub enum StorageConfig {
    S3(S3Config),                           // matches backend: s3
    #[config(default)]                      // used when the tag key is absent
    Filesystem(FilesystemConfig),
}
```

```yaml
storage:
  backend: s3        # selects S3; S3Config parses from "storage.*"
  bucket: my-bucket
```

- Variants are **unit** or **single-field tuple** (field: `ConfigProperties`).
- Tag matching: snake_case of the variant name by default;
  `#[config(rename_all = "lowercase" | "snake_case" | "kebab-case")]` on the
  enum, `#[config(rename = "...")]` per variant.
- `#[config(default)]` on one variant: constructed when the tag key is
  absent. Without it, a missing tag is `ConfigError::NotFound`.
- An unknown tag value is `ConfigError::Deserialize` listing the expected values.
- `Children = TNil` (variant payloads are not auto-registered as beans);
  metadata is the tag key only — payload validation happens via `from_config`.

For **data-shaped** enums (a scalar value like `mode: strict`), keep using
`#[derive(FromConfigValue)]` via serde instead.

### Manual usage (outside injection)

```rust
let config = R2eConfig::load()?;
let db = DatabaseConfig::from_config(&config, Some("app.database"))?;
```

### Generated trait

```rust
trait ConfigProperties {
    type Children;  // type-level list of nested section types; NoChildren for leaves
    fn from_config(config: &R2eConfig, prefix: Option<&str>) -> Result<Self, ConfigError>;
    fn properties_metadata(prefix: Option<&str>) -> Vec<PropertyMeta> { vec![] }  // default no-op
    fn register_children(&self, registry: &mut BeanRegistry) {}  // default no-op
}
```

### Supported manual implementations

When the derive can't express a shape, a manual impl is **supported public
API** — only `type Children` and `from_config` are required:

```rust
use r2e::prelude::*;   // ConfigProperties, NoChildren, PropertyMeta, R2eConfig, ConfigError

impl ConfigProperties for MyDynamicConfig {
    type Children = NoChildren;

    fn from_config(config: &R2eConfig, prefix: Option<&str>) -> Result<Self, ConfigError> {
        // build from config.sub_keys(...) / config.has_prefix(...) / config.get(...)
    }
}
```

`R2eConfig::has_prefix(prefix)` (any key under the prefix, empty YAML key =
absent) and `R2eConfig::sub_keys(prefix)` (distinct immediate child segments,
sorted) are the supported primitives for dynamic shapes. Do not reach into
`r2e::type_list` or `r2e::config::typed` internals.

`register_children` is generated by the derive macro. For each `#[config(section)]` field, it calls `registry.provide(child.clone())` and recursively `child.register_children(registry)`. For `Option<T>` sections, it only registers when `Some`. This enables `load_config::<Root>()` to automatically register all nested config types as beans.

**Optional sections are not `#[inject]`-able**: an `Option<T>` section is excluded from the compile-time `Children` list (it may be absent), so `#[inject] tls: TlsConfig` won't resolve — inject the parent config and read the `Option` field instead. Map-valued sections are likewise not registered as beans (all entries share one type).

---

## Injection

### `#[config("key")]` — single value

Reads one scalar from `R2eConfig`. The key is a full dot-separated path. Type must implement `FromConfigValue`.

### `#[inject]` — typed config section (via `load_config`)

When using `load_config::<RootConfig>()`, all `#[config(section)]` children are auto-registered as beans. Inject them directly with `#[inject]`:

```rust
#[controller(path = "/my")]
pub struct MyController {
    #[inject]
    root_config: RootConfig,          // injected from state (auto-registered by load_config)

    #[config("app.name")]
    name: String,                     // single scalar value
}
```

### `#[config_section(prefix = "...")]` — legacy typed section

Reads an entire struct from `R2eConfig` at request time. Type must implement `ConfigProperties`. Still supported for backward compatibility but `#[inject]` with `load_config` is preferred.

### When to use which

| Situation | Use |
|---|---|
| 1–2 isolated values from different sections | `#[config("full.key")]` |
| Typed config section (registered via `load_config`) | `#[inject]` on the config type |
| Config needed outside DI (main, tests) | `ConfigProperties::from_config()` |

### In controllers

```rust
#[controller(path = "/my")]
pub struct MyController {
    #[config("app.name")]
    name: String,

    #[inject]
    root_config: RootConfig,       // auto-registered by load_config
}
```

### In beans and producers

```rust
#[bean]
impl SearchService {
    fn new(
        #[config("app.name")] name: String,
        matching: MatchingConfig,           // resolved from BeanContext (auto-registered child)
        other_dep: OtherDep,
    ) -> Self { ... }
}

#[producer]
fn create_search(m: MatchingConfig) -> SearchService { ... }  // MatchingConfig from BeanContext
```

---

## AppBuilder integration

Two pre-state methods to provide config (call before `.build_state()` or `.with_state()`):

### `load_config::<C>()` — load + provide (recommended)

The idiomatic way to set up configuration. Loads YAML + env, stores the raw config in the builder, and provides `R2eConfig` in the bean registry. If `C` is not `()`, also constructs the typed config, **auto-registers all nested `#[config(section)]` children as beans** (via `register_children`), and provides both `C` and `R2eConfig` in the compile-time type list.

```rust
AppBuilder::new().load_config::<()>()           // raw config only
AppBuilder::new().load_config::<RootConfig>()   // raw + typed + children (preferred)
```

`C` must implement `LoadableConfig` — satisfied by `()` (raw only) and any `T: ConfigProperties`.

When using a root config with nested sections, all children are available for `#[inject]` in controllers and as bean dependencies:

```rust
#[derive(ConfigProperties, Clone, Debug)]
pub struct RootConfig {
    #[config(section)]
    pub app: AppConfig,          // auto-registered as a bean
    #[config(section)]
    pub database: DatabaseConfig, // auto-registered as a bean
}
```

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

## Well-known Config Keys

### Server

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `server.host` | `String` | `"0.0.0.0"` | Bind host for TCP and QUIC |
| `server.port` | `u16` | `3000` | TCP port |
| `server.tcp_nodelay` | `bool` | `true` | Set `TCP_NODELAY` on every accepted TCP connection. Disabling Nagle's algorithm reduces latency on small responses at the cost of slightly higher network overhead. |
| `server.workers` | `usize` \| `"per-core"` | *(absent → single listener)* | SO_REUSEPORT sharded serving: `N` worker threads, each a `current_thread` runtime with its own listener on the same address. `"per-core"` = `available_parallelism()`. `0`/negative/unknown string → error. Unix only (excl. solaris/illumos/cygwin). Unsupported with `dev-reload` (ignored, warns). See `docs/features/19-sharded-serving.md`. |
| `server.quic.port` | `u16` | — | UDP port for QUIC/HTTP3 (enables QUIC when set) |
| `server.quic.cert` | `String` | — | PEM certificate chain path (required with `quic.port`) |
| `server.quic.key` | `String` | — | PEM private key path (required with `quic.port`) |
| `server.quic.alt_svc_max_age` | `u32` | `3600` | Alt-Svc header max-age in seconds |

---

## Reference

### ConfigError

```rust
enum ConfigError {
    NotFound(String),
    TypeMismatch { key: String, expected: &'static str },
    Load(String),
    Deserialize { key: String, message: String },  // serde deserialization (FromConfigValue derive)
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

### Built-in ConfigProperties types

`TracingConfig` (in `r2e-core::tracing_config`) — configurable tracing subscriber options (filter, format, ansi, thread IDs, etc.). Used by the `ConfiguredTracing` plugin and `ObservabilityConfig`. Read from YAML under a prefix (e.g., `tracing.*` or `observability.tracing.*`).

Related enums: `LogFormat` (`pretty` / `json`), `SpanEvents` (`none` / `new` / `close` / `active` / `full`). Both derive `FromConfigValue` via serde.

### Prelude exports

`R2eConfig`, `ConfigProperties`, `ConfigValue`, `ConfigError`, `ConfigValidationDetail`, `FromConfigValue`, `FromConfigValue` (derive macro), `NoChildren`, `PropertyMeta`, `deserialize_value`, `SecretResolver`, `DefaultSecretResolver`, `TracingConfig`, `LogFormat`, `SpanEvents`.
