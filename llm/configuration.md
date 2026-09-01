---
topic: configuration
features: core
tokens: ~3500
requires: di-beans
---

## Configuration

### TL;DR

- Resolution order, lowest → highest: `application.yaml` (+ `application-{profile}.yaml`)
  → `.env` → `${VAR}` placeholders → `.with_config_provider(..)` providers →
  provider placeholders → `R2E_` env vars.
- The `R2E_` overlay is **strict** (`_`→`.`, nothing else): a key containing `-` or
  an in-segment `_` is not addressable that way — use a `${VAR}` placeholder in
  YAML for it.
- Prefer `.load_config::<RootConfig>()`: it builds the typed tree and registers the
  root plus every `#[config(section)]` child as a bean, injected by type with
  `#[inject]`.
- `#[config("dotted.key")]` reads a single scalar; a missing **required** key fails
  at startup, while an `Option<T>` field resolves to `None`.
- Two `#[config(section)]` fields of the **same type** are a compile error —
  sections are provided by type and there are no bean qualifiers, so give the second
  its own type.
- `provide_config(value)` is the test/embedding counterpart of `load_config`: it
  also registers the nested sections, which a plain provide of the struct does not.
- Values that rotate at runtime need `#[live_config("key")] x: LiveConfig<T>` read
  through `get()` (a cached read — per request is fine); `#[config]` fields and typed
  config beans never see rotations.
- Combining `#[live_config]` with `#[config]`, `#[config_section]`, `#[inject]`, or a
  request-scoped `#[inject(identity)]`/`#[inject(request)]` field is a compile error.
- Copied keys (`#[config]`, sections, `ConfigProperties`) are presence-validated and
  fingerprinted for `r2e dev` rebuilds; live keys are neither — a typo'd live key
  only logs a `WARN`.
- `#[config(derive_default)]` emits `Default` from the declared defaults; a required
  field in such a struct is a compile error.

### Files & resolution order (lowest → highest)

1. `application.yaml` (hierarchies flattened to dotted keys) + `application-{profile}.yaml` (profile from `R2E_PROFILE`)
2. `.env` file
3. `${VAR}` / `${VAR:default}` / `${file:/path}` secret placeholders in string values
4. Config providers registered with `.with_config_provider(...)`
5. Provider-supplied string placeholders are resolved again
6. Env vars prefixed `R2E_` (`R2E_SERVER_PORT=8080` ↔ `server.port`). The
   mapping is strict (`_`→`.`, nothing else — no fuzzy matching): a key
   containing `-` or an in-segment `_` (`security.jwt.jwks-url`,
   `database.max_idle`) is NOT addressable via any `R2E_` var. For env-driven
   values of such keys use a `${VAR}` placeholder in YAML
   (`jwks-url: "${JWKS_URL:}"` → set `JWKS_URL`, unprefixed). Other unprefixed
   env vars are ignored.

### `load_config` — the idiomatic path

```rust
#[derive(ConfigProperties, Clone, Debug)]
pub struct RootConfig {
    #[config(section)]
    pub app: AppConfig,                   // auto-registered as a bean
    #[config(section)]
    pub database: DatabaseConfig,         // auto-registered as a bean
}

#[derive(ConfigProperties, Clone, Debug)]
pub struct DatabaseConfig {
    pub url: String,                      // required — startup error if missing
    #[config(default = 5)]
    pub pool_size: i64,
}

// in App::build:
# fn __doc(b: AppBuilder) -> impl Sized {
b.load_config::<RootConfig>()             // typed + children as beans
# }
// controllers/beans then simply:
# #[controller(path = "/demo")]
# pub struct DemoController {
#[inject] db_config: DatabaseConfig,      // typed section, injected by type
#[config("app.greeting")] greeting: String,  // single scalar
# }
# fn main() {}
```

### `provide_config` — a typed config struct already in hand

`provide(settings)` puts only the struct itself in the graph, so a nested
`#[config(section)]` child stays invisible and every injector of that child
needs a hand-written producer. `provide_config` also runs `register_children`,
landing the parent **and** every nested section in the graph — the same set
`load_config::<C>()` provides, minus the disk read, the `R2eConfig` bean and
the live-config registry:

```rust
# async fn __doc(d: AppSettings) -> impl Sized {
AppBuilder::new()
    .provide_config(AppSettings { db: DatabaseSettings { url: ":memory:".into() }, ..d })
    .build_state().await
    .register_controller::<UserController>()   // #[inject] db: DatabaseSettings resolves
# }
```

It is the test/embedding counterpart of `load_config` (same builder phase). The
value is pinned like any `provide`d bean: nothing rebuilds it from `R2eConfig`
on a dev-reload cycle.

### Config providers and rotation

Implement `ConfigProvider` for external sources such as Vault and register it
before `load_config`:

```rust
# fn __doc(b: AppBuilder) -> impl Sized {
b.with_config_provider(VaultConfigProvider::new("https://vault.internal:8200"))
 .load_config::<RootConfig>()
# }
```

Provider `load` mutates the boot `R2eConfig` before typed config is built.
`ConfigProvider::watch` is **supervised**: returning `Err(_)` is a broken watch
(logged, then retried with capped exponential backoff), returning `Ok(())`
means "done watching, never call me again" (the default impl and one-shot
providers). A long-lived watcher runs until
`ConfigWatchContext::shutdown_token()` (a `r2e::rt::CancelToken`) fires and
returns `Ok(())` then.
Runtime rotations do not mutate app-scoped `#[config]` fields or typed config
beans; use the automatically provided `LiveConfigRegistry` / `LiveConfig<T>`
handles for live values. This is plain live/dynamic config (feature flags,
timeouts, URLs, credentials alike) — it is not the `${...}` secret-placeholder
mechanism:

```rust
#[producer(start)]
async fn create_client(
    #[live_config("search.endpoint")] endpoint: LiveConfig<String>,
) -> SearchClient {
    SearchClient::connect(endpoint).await.unwrap()
}
# fn main() {}
```

(A database pool is the one live-config consumer you do **not** hand-roll: the
datasource plugin — `.plugin(SqlxDataSource::<sqlx::Postgres>::new())` — reads
`datasource.url` as a live value and hands back a rotating `DbPool`.)

`#[live_config("key")]` is also a field attribute, symmetric with
`#[config("key")]` — on `#[derive(Bean)]` / `#[derive(DecoratorBean)]` /
`#[derive(BackgroundService)]` structs, `#[bean]` constructor params, and
`#[controller]` structs:

```rust
#[derive(Clone, Bean)]
pub struct PricingService {
    #[live_config("pricing.multiplier")] multiplier: LiveConfig<f64>,
}

#[controller(path = "/pricing")]
pub struct PricingController {
    #[live_config("pricing.multiplier")] multiplier: LiveConfig<f64>,
}

#[routes]
impl PricingController {
    #[get("/")]
    async fn rate(&self) -> String {
        self.multiplier.get().unwrap_or(1.0).to_string()
    }
}
# fn main() {}
```

The field is app-scoped like `#[config]` (resolved once — bean `build()`,
controller `register_controller()`); freshness comes from `get()`, not from
re-resolution. `LiveConfigRegistry` joins the host's dependency list, so a
missing `load_config` is a normal missing-bean error. Combining `#[live_config]`
with `#[config]`, `#[config_section]`, `#[inject]`, or a request-scoped
`#[inject(identity)]` / `#[inject(request)]` field is a compile error.

`LiveConfig<T>`: `get() -> Result<T, ConfigError>`, `snapshot() ->
LiveConfigSnapshot` (versioned), `subscribe() -> LiveConfigReceiver<T>`. `get()`
is a cached read — the slot is bound once at handle creation and the typed
conversion only reruns when the version moved — so calling it per request is
fine. Registry slots are created lazily on first access (seeded from the boot
config when the key existed), not one per config entry at load.
Provider watchers call `ConfigUpdateSink::set(key, value)` during serve-time
(`ConfigUpdateSink::registry()` returns the `LiveConfigRegistry`). A
`#[live_config]` key is never presence-checked at startup (the value may
legitimately be absent at boot) and never fingerprinted for dev-reload — see
"Copied vs subscribed" below. Because of that, a *typo'd* key would fail
silently: creating a handle for a key that has no value and whose app registered
no `ConfigProvider` logs a `WARN` naming the key (predicate:
`LiveConfigRegistry::is_dead_key`). Keys overridden by
`override_config_value` or `R2E_...` env vars are pinned and ignore runtime
provider updates — including when `override_config_value` runs *after*
`load_config` (it patches and pins the live slot too). A late
`override_config_value` does **not** rebuild typed `ConfigProperties` sections
already constructed by `load_config`; override before `load_config` for those.

**Copied vs subscribed.** Every config key a component declares uses one of two
freshness modes, recorded in its `config_keys()` entry
`(key, type_name, ConfigKeyKind)`:

- **Copied** — `#[config]`, `#[config_section]`, `ConfigProperties`,
  `config.get::<T>(…)`. Read once at construction, stored by value.
  `ConfigKeyKind::Required` (presence-validated at startup),
  `ConfigKeyKind::Optional` (an `Option<T>` field; not validated), or
  `ConfigKeyKind::Section` (a `#[config_section(prefix = "p")]` field — the
  entry's key is the **prefix**, so it is not presence-checked either: a
  section is validated as a whole: a controller/gRPC core walks its own
  sections in the generated `validate_config` at registration, a bean walks
  its own when it is constructed inside `build_state()`, and the two hosts
  that construct *late* declare `SectionValidator`s instead —
  `DecoratorSpec::config_sections()` (guards/interceptors) and
  `ServiceComponent::config_sections()` (background services), both run
  through `validate_declared_sections` by the registration path that owns
  them). **All three are
  fingerprinted**: under `r2e dev`, editing the key — or, for a section, *any*
  key under its prefix — rebuilds the declaring bean and its dependents.
- **Subscribed** — `#[live_config]` → `LiveConfig<T>`. The handle binds a
  registry slot; new values are pushed in. `ConfigKeyKind::Live`: never
  presence-validated, **never fingerprinted** (a rebuild would produce an
  identical handle). Under `r2e dev` the `LiveConfigRegistry` keeps one stable
  identity for the session and is re-seeded from the fresh config on each hot
  patch, so a live-key edit reaches existing handles with no bean rebuilt.

`ConfigKeyKind::is_required()` drives startup validation; `is_fingerprinted()`
(false only for `Live`) drives dev-reload invalidation; `is_prefix()` (true only
for `Section`) makes the fingerprint hash the whole subtree via
`R2eConfig::prefix_fingerprint(prefix)`. `Optional` is a copied kind — "not
required" is not the same as "live".

Net dev-reload behaviour: **live edit → push** (nothing rebuilt), **copied edit
→ rebuild** (declaring bean + dependents).

Field attributes: `#[config(default = ...)]`, `#[config(key = "...")]`,
`#[config(env = "VAR")]`, `#[config(section)]` (+ `Option<Section>` =
presence-based, `HashMap<String, T>` = map-valued section), `#[config(skip)]`.
Tagged enum sections: `#[config(tag = "backend")]` on an enum selects the
variant from a config key. Enums as scalar values: `#[derive(FromConfigValue)]`
via serde.

Two `#[config(section)]` fields of the **same type** are a compile error: the
generated `register_children` provides sections as beans **by type**, so the
second would silently overwrite the first and every `#[inject]` of that type
would read whichever won. R2E has no bean qualifiers — give the second section
its own type (the diagnostic suggests a name).

Struct attribute `#[config(derive_default)]` — opt-in, emits `impl Default`
from the declared defaults, so an app never restates them in a hand-written
impl:

```rust
#[derive(ConfigProperties, Clone)]
#[config(derive_default)]
pub struct HttpConfig {
    #[config(default = 8080)] pub port: u16,
    #[config(default = "0.0.0.0")] pub host: String,
    pub tls_cert: Option<PathBuf>,
}
// HttpConfig::default() == HttpConfig::from_config(&R2eConfig::empty(), None).unwrap()
```

That equality is the contract, so a **required** field (no `default`, not
`Option<T>`, not `skip`, not `#[config(section, default)]`/map section) is a
compile error rather than a silent `Default::default()` that would disagree
with what config loading produces. It is opt-in: a struct with a hand-written
`Default` is untouched.

Missing **required** `#[config]` keys fail at startup (controller registration
validates them). The message names the full working `R2E_…` env var (prefix
included) for purely dotted keys; a key containing `-` or `_`
(`database.min-idle`, `database.max_idle`) is not addressable via the strict
`R2E_` overlay, so its message points at `application.yaml` / `${VAR}`
placeholders instead.

`load_config::<C>()` validates the whole typed tree **before** constructing it
(`LoadableConfig::validate`, the trait's second method alongside `register`), so
one boot reports **every** missing key of `C` and of its nested sections at
once — not just the first one `from_config` trips over. As with a missing or
malformed config file, the failure is parked on the builder (the type-state
transition cannot return a `Result`) and surfaces from `try_build_state()` as
`BeanError::MissingConfigKeys`, before any bean is constructed;
`build_state()` panics with the same rendered report.
`Option<T>` `#[config]` fields are optional — a missing key resolves to `None`
and is never reported as missing.

### Well-known keys

`server.host` (default `0.0.0.0`), `server.port` (default `3000`) — read by
`serve_auto()`. `server.workers` (`N` or `"per-core"`) — SO_REUSEPORT sharded
serving (required by `per_worker_service`). `server.drain-timeout` (duration,
default `30s`) — HTTP drain budget at shutdown; `.drain_timeout(d)` /
`.drain_timeout_unbounded()` on the builder win over it. `server.tcp_nodelay`
(default `true`). `server.quic.*` — HTTP/3. `services.enabled` (default `true`)
— global background-service switch; `false` keeps every `ServiceComponent` out
of `run()` while leaving registration and config validation untouched.
