# #1180 — Serde-only typed config (`SerdeConfig<T>`)

Tasker: #1180 "Config typée : charger une config serde existante via
`load_config::<T>()` sans dépendre de r2e". Status: **plan, not started**.

## 1. What the ticket asks vs. what the framework is missing

The ticket is written from llm-engine-rust's workaround (`settings.rs`: a
hand-rolled flat-keys → JSON tree bridge + a serde field probe) and proposes
four pistes (A wrapper, B tree API, C serde error paths, D richer derive).
Before picking any of them, restate the need in framework terms:

| Ticket symptom | Underlying gap in r2e |
|---|---|
| Can't `#[derive(ConfigProperties)]` in a framework-free crate | Typed config has exactly one entry door, the derive. A type that already knows how to deserialize itself (serde) has no way in. |
| Derive lacks `untagged`, `default = "fn"`, enums in `Vec<struct>` | Same gap: the derive is a *second* schema language. Serde already covers these shapes; r2e does not need to re-grow them. |
| Bridge must probe serde field names because root keys can't be listed | `R2eConfig` has `has_prefix`/`sub_keys` (one level) but no "give me the subtree" primitive. Flat map is the storage; the tree view is missing. |
| Bridge reads the parent `List` and misses `engines.0.key` overrides | Undocumented invariant: **flat keys are the source of truth**, the parent `List` is a convenience copy. Any tree reconstruction must merge indexed keys over it. |
| Resolved `${VAR}` and `R2E_*` values are strings → serde `u16` fails | `FromConfigValue` coerces strings into numbers/bools; a serde path that does not would make `R2E_*` overrides second-class for one config style only. |
| Lost `config_derived_scope`, test overrides | Not gaps — both are properties of `LoadableConfig for T: ConfigProperties`. Anything that *is* a `ConfigProperties` gets them for free. |

Conclusion: the right shape is not a new loader (`load_serde_config`) nor a
third `LoadableConfig` impl. It is **one newtype that implements
`ConfigProperties`**, built on **one new `R2eConfig` primitive**. Everything
else (`load_config`, `provide_config`, `PluginConfig`, `#[config(section)]`,
`validate_section`, dev-reload exemption, `override_config_value`) already
flows from `ConfigProperties`.

Piste D (richer derive) is **rejected for this ticket**: once serde types are
first-class, the derive stays the opinionated path for r2e-native structs and
does not need to chase serde's feature set. Reopen only if a derive user asks.

## 2. Design

### 2.1 `R2eConfig::tree(prefix: Option<&str>) -> Option<ConfigValue>` (piste B)

Third dynamic-shape primitive next to `has_prefix` / `sub_keys`. Rebuilds the
subtree under `prefix` (whole config for `None`) as nested `ConfigValue::Map`
/ `List` / leaves, **from the flat keys**:

- Node with no dotted children → its leaf value (`None` if absent).
- Node with children → `Map`, unless it is a list (below).
- **List detection**: the exact key holds a `ConfigValue::List` (YAML
  sequence) **or** every child segment is a decimal integer and they form
  `0..n` (env-only lists). The parent `List` seeds length + element values;
  flat `key.N[.rest]` entries then override element by element, so
  `R2E_ENGINES_0_KEY=…` and `override_config_value("engines.0.key", …)` are
  visible. An empty YAML list (`[]`, no indexed keys) still yields `List([])`.
- Exact key holds a scalar **and** has children (`R2E_A=1` next to `a.b`):
  children win, scalar dropped — same reading as `has_prefix`, which already
  treats a `Null` at the exact key as "absent section".
- `Null` at a key with no children → `Some(Null)` (serde maps it to `None` /
  default); a bare empty section (`tls:`) therefore deserializes like an
  absent one.

Pure function over the map, no allocation surprises beyond the tree itself;
only called at boot (and per dev-reload cycle). Replaces llm-engine's
`subtree` + `struct_fields` outright.

### 2.2 `SerdeConfig<T>` — a `ConfigProperties` newtype (piste A, reshaped)

```rust
// r2e_core::config
#[derive(Clone, Debug)]
pub struct SerdeConfig<T>(pub T);          // Deref<Target = T> + into_inner()

impl<T> ConfigProperties for SerdeConfig<T>
where T: DeserializeOwned + Clone + Send + Sync + 'static
{
    type Children = TCons<T, TNil>;        // T itself is an injectable bean

    fn from_config(config: &R2eConfig, prefix: Option<&str>) -> Result<Self, ConfigError> {
        let tree = config.tree(prefix).unwrap_or(ConfigValue::Map(Default::default()));
        deserialize_tree::<T>(&tree, prefix).map(SerdeConfig)   // §2.3 + §2.4
    }

    fn register_children(&self, registry: &mut BeanRegistry) {
        registry.provide(self.0.clone());
    }
    // properties_metadata: default (empty) — serde has no schema to enumerate.
}
```

Coherence: verified (`impl<T: ConfigProperties> LoadableConfig for T` and
`PluginConfig for T` coexist with `impl ConfigProperties for SerdeConfig<T>`
because both the type and the trait are local).

What falls out without any new code:

| Surface | Behaviour |
|---|---|
| `AppBuilder::load_config::<SerdeConfig<AppConfig>>()` | validates (empty report), constructs inside `config_derived_scope` → dev-reload exempt; provides `SerdeConfig<AppConfig>` **and** `AppConfig` (via `Children`); `override_config_value` / `R2E_*` / test profile already applied before `register`. |
| `#[inject] cfg: AppConfig` in beans/controllers | resolves through `Children`. |
| `provide_config(SerdeConfig(cfg))` | in-memory variant, same bean set. |
| `Plugin::Config = SerdeConfig<MyCfg>` | prefix comes from `CONFIG_PREFIX`; blanket `PluginConfig` applies. |
| `#[config(section)] engine: SerdeConfig<EngineCfg>` inside a derived struct | prefixed serde island inside an r2e-native tree; registered as a child bean like any section. |
| `SerdeConfig::<T>::from_config(&cfg, Some("app"))` | manual / tests. |

Known, documented trade-offs (not gaps to fill now):
- **No aggregated missing-key report**: serde stops at the first error, so a
  boot with three missing fields takes three boots. `ConfigProperties`
  derive keeps that advantage; it is the price of serde-only types.
- **No `PropertyMeta`**: `r2e doctor` / docs listing cannot enumerate the
  fields of a `SerdeConfig<T>` section.

### 2.3 Error key naming (piste C)

`deserialize_tree` runs the deserializer through `serde_path_to_error`
(already in `Cargo.lock` via axum — no new dependency weight) and maps the
path `engines[1].backends[0].weight` to the r2e key
`<prefix>.engines.1.backends.0.weight` in `ConfigError::Deserialize { key,
message }`. A `missing field` error carries the parent path as `key` and the
field name in `message` — good enough, do not parse serde messages.

### 2.4 String coercion — `ConfigValue` serde `Deserializer`

`deserialize_value` today goes `ConfigValue → serde_json::Value → T`, and
`serde_json::Value` does not coerce `"8080"` into `u16`. Since **every**
`R2E_*` override and every resolved `${VAR}` is a `String`, a serde-typed
config would silently lose env overrides on every non-string field — the one
behaviour that must be identical across config styles.

Implement `serde::Deserializer` for `&ConfigValue` (private module
`config/de.rs`): structural variants behave like `serde_json::Value`'s
deserializer; the typed hints `deserialize_{i*,u*,f*,bool}` on a
`ConfigValue::String` parse it with the same rules as `FromConfigValue`
(`"true"/"1"/"yes"` …). `deserialize_any` never coerces (so `untagged` and
`serde_json::Value` fields see the real string). `deserialize_value` switches
to it, so `#[derive(FromConfigValue)]` enums/structs get the same semantics —
strictly broader acceptance, no behaviour removed.

This is the largest piece (~250 lines + tests). It is a separate step and
can ship in a follow-up PR if the first PR has to land fast; the first PR
then documents "resolved placeholders stay strings" as the ticket allows.

## 3. Steps

1. **`R2eConfig::tree`** in `config/mod.rs` + tests in
   `r2e-core/tests/config/` (new `mod tree`): YAML map/list/scalar, empty
   list, indexed override over parent list, env-only list, scalar-with-children,
   `Null` section, root (`None`).
2. **`SerdeConfig<T>`** in `config/serde_config.rs` (+ `deserialize_tree` with
   `serde_path_to_error`), re-exported from `r2e_core::config` and the
   prelude; tests `mod serde_config`: `load_config` integration (`#[inject]
   AppConfig`), `override_config_value` visible, `R2E_*` visible, nested
   section usage, plugin `Config`, error key for a nested `Vec<struct>` field,
   dev-reload exemption (mirror the existing `config_derived` test for
   `ConfigProperties`).
3. **`ConfigValue` deserializer** (`config/de.rs`), `deserialize_value`
   switched; tests `mod de`: coercion matrix, `deserialize_any` passthrough,
   untagged enum, option/null, nested enum in `Vec`.
4. **Docs**: `docs/claude/configuration.md` (new section "Serde-only config
   types" after "Supported manual implementations"; add `tree` to the Methods
   table; state the flat-keys-are-truth invariant), `llm/configuration.md`
   spoke (compilable example, `rust` not `rust,ignore`), then
   `scripts/check-llm-docs.sh --update`.
5. **Changelog**: `r2e-core/CHANGELOG.md` `[Unreleased]` → `Added` (additive,
   **patch** bump at release time; no facade bump needed beyond the usual
   re-pin).
6. **Ticket**: tick the acceptance boxes, note the two documented trade-offs,
   and open the llm-engine-rust follow-up (drop `settings::app_config` /
   `struct_fields` / `subtree`, `ConfigModule`, the producer; call
   `.load_config::<SerdeConfig<AppConfig>>()`; keep `sync_server_keys` or
   move `host`/`port` under `server.*`).

Out of scope, by decision: piste D (derive features), aggregated validation
for serde types, `r2e doctor` enumeration of serde sections.
