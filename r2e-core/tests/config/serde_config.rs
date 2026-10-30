//! `SerdeConfig<T>`: serde-only typed config sections.

use std::sync::{Arc, Mutex};

use r2e_core::config::{ConfigError, ConfigProperties, R2eConfig, SerdeConfig};
use r2e_core::plugin::{Plugin, PluginBuildContext, PluginBuildError};
use r2e_core::prelude::*;
use r2e_core::BeanAccess as _;
use serde::Deserialize;

// ── Fixtures: plain serde types, no r2e derive ─────────────────────────────

#[derive(Deserialize, Clone, Debug, PartialEq)]
struct Backend {
    url: String,
    #[serde(default = "default_weight")]
    weight: i64,
}

fn default_weight() -> i64 {
    1
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Engine {
    Local { key: String },
    Remote { key: String, backends: Vec<Backend> },
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
struct Settings {
    name: String,
    #[serde(default)]
    engines: Vec<Engine>,
    #[serde(default)]
    tags: Vec<String>,
    region: Option<String>,
}

const YAML: &str = r#"
app:
  name: catalog
  tags: [a, b]
  engines:
    - kind: local
      key: l1
    - kind: remote
      key: r1
      backends:
        - url: http://one
          weight: 3
        - url: http://two
"#;

fn expected() -> Settings {
    Settings {
        name: "catalog".into(),
        engines: vec![
            Engine::Local { key: "l1".into() },
            Engine::Remote {
                key: "r1".into(),
                backends: vec![
                    Backend {
                        url: "http://one".into(),
                        weight: 3,
                    },
                    Backend {
                        url: "http://two".into(),
                        weight: 1,
                    },
                ],
            },
        ],
        tags: vec!["a".into(), "b".into()],
        region: None,
    }
}

// ── from_config ────────────────────────────────────────────────────────────

#[test]
fn from_config_reads_the_prefixed_section() {
    let config = R2eConfig::from_yaml_str(YAML).unwrap();
    let cfg = SerdeConfig::<Settings>::from_config(&config, Some("app")).unwrap();
    assert_eq!(*cfg, expected());
    assert_eq!(cfg.into_inner().name, "catalog");
}

#[test]
fn from_config_at_root_ignores_unknown_keys() {
    let config = R2eConfig::from_yaml_str("name: root\nunrelated:\n  key: 1\n").unwrap();
    let cfg = SerdeConfig::<Settings>::from_config(&config, None).unwrap();
    assert_eq!(cfg.name, "root");
    assert!(cfg.engines.is_empty());
}

#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
struct AllDefaults {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    items: Vec<String>,
}

#[test]
fn absent_or_null_section_deserializes_from_an_empty_map() {
    let config = R2eConfig::from_yaml_str("other: 1\nempty:\n").unwrap();
    let absent = SerdeConfig::<AllDefaults>::from_config(&config, Some("missing")).unwrap();
    assert_eq!(absent.0, AllDefaults::default());
    let null = SerdeConfig::<AllDefaults>::from_config(&config, Some("empty")).unwrap();
    assert_eq!(null.0, AllDefaults::default());
}

#[test]
fn indexed_override_is_visible() {
    let mut config = R2eConfig::from_yaml_str(YAML).unwrap();
    config.set("app.engines.1.backends.1.url", "http://patched".into());
    let cfg = SerdeConfig::<Settings>::from_config(&config, Some("app")).unwrap();
    match &cfg.engines[1] {
        Engine::Remote { backends, .. } => assert_eq!(backends[1].url, "http://patched"),
        other => panic!("unexpected engine {other:?}"),
    }
}

#[test]
fn r2e_env_overlay_is_visible() {
    let _env = crate::support::env_lock();
    crate::support::set_env("R2E_APP_ENGINES_0_KEY", "from-env");
    crate::support::set_env("R2E_APP_REGION", "eu-west");

    let mut config = R2eConfig::from_yaml_str(YAML).unwrap();
    config.apply_current_env_overlay();

    crate::support::remove_env("R2E_APP_ENGINES_0_KEY");
    crate::support::remove_env("R2E_APP_REGION");

    let cfg = SerdeConfig::<Settings>::from_config(&config, Some("app")).unwrap();
    assert_eq!(
        cfg.engines[0],
        Engine::Local {
            key: "from-env".into()
        }
    );
    assert_eq!(cfg.region.as_deref(), Some("eu-west"));
}

// ── Error keys ─────────────────────────────────────────────────────────────

fn deserialize_error(yaml: &str, prefix: Option<&str>) -> (String, String) {
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    match SerdeConfig::<Settings>::from_config(&config, prefix) {
        Err(ConfigError::Deserialize { key, message }) => (key, message),
        other => panic!("expected a Deserialize error, got {other:?}"),
    }
}

#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)]
struct Pool {
    name: String,
    backends: Vec<Backend>,
}

#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)]
struct Cluster {
    pools: Vec<Pool>,
}

const CLUSTER_YAML: &str = r#"
cluster:
  pools:
    - name: p0
      backends:
        - url: http://a
    - name: p1
      backends:
        - url: http://b
          weight: 2
        - url: http://c
"#;

fn cluster_error(yaml: &str) -> (String, String) {
    let config = R2eConfig::from_yaml_str(yaml).unwrap();
    match SerdeConfig::<Cluster>::from_config(&config, Some("cluster")) {
        Err(ConfigError::Deserialize { key, message }) => (key, message),
        other => panic!("expected a Deserialize error, got {other:?}"),
    }
}

#[test]
fn error_key_names_the_nested_vec_struct_field() {
    let yaml = CLUSTER_YAML.replace("weight: 2", "weight: [nope]");
    let (key, message) = cluster_error(&yaml);
    assert_eq!(key, "cluster.pools.1.backends.0.weight");
    assert!(message.contains("invalid type"), "{message}");
}

#[test]
fn missing_field_error_carries_the_parent_path() {
    let yaml = CLUSTER_YAML.replace("        - url: http://c\n", "        - weight: 3\n");
    let (key, message) = cluster_error(&yaml);
    assert_eq!(key, "cluster.pools.1.backends.1");
    assert!(message.contains("missing field `url`"), "{message}");
}

/// Internally tagged / untagged enums are buffered by serde before dispatch,
/// so the path stops at the enum element itself — still the right list item.
#[test]
fn error_key_stops_at_a_buffered_enum() {
    let yaml = YAML.replace("weight: 3", "weight: [nope]");
    let (key, message) = deserialize_error(&yaml, Some("app"));
    assert_eq!(key, "app.engines.1");
    assert!(message.contains("invalid type"), "{message}");
}

#[test]
fn error_key_falls_back_to_prefix_then_root() {
    let (key, message) = deserialize_error("app:\n  tags: []\n", Some("app"));
    assert_eq!(key, "app");
    assert!(message.contains("missing field `name`"), "{message}");

    let (key, _) = deserialize_error("tags: []\n", None);
    assert_eq!(key, "<root>");
}

// ── AppBuilder integration ─────────────────────────────────────────────────

#[derive(Deserialize, Clone, Debug)]
struct RootSettings {
    name: String,
    #[serde(default)]
    engines: Vec<Engine>,
}

#[derive(Clone)]
struct Greeter {
    greeting: String,
}

#[bean]
impl Greeter {
    fn new(settings: RootSettings) -> Self {
        Self {
            greeting: format!("hello {}", settings.name),
        }
    }
}

#[r2e_core::test]
async fn load_config_provides_wrapper_and_inner_type() {
    let config = R2eConfig::from_yaml_str("name: catalog\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<SerdeConfig<RootSettings>>()
        .register::<Greeter>()
        .try_build_state()
        .await
        .expect("serde config binds");

    let state = app.state();
    assert_eq!(state.get::<SerdeConfig<RootSettings>>().name, "catalog");
    assert_eq!(state.get::<RootSettings>().name, "catalog");
    assert_eq!(state.get::<Greeter>().greeting, "hello catalog");
}

#[r2e_core::test]
async fn override_config_value_reaches_the_serde_section() {
    let config =
        R2eConfig::from_yaml_str("name: catalog\nengines:\n  - kind: local\n    key: yaml\n")
            .unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .override_config_value("engines.0.key", "overridden")
        .load_config::<SerdeConfig<RootSettings>>()
        .try_build_state()
        .await
        .expect("serde config binds");

    assert_eq!(
        app.state().get::<RootSettings>().engines[0],
        Engine::Local {
            key: "overridden".into()
        }
    );
}

#[r2e_core::test]
async fn serde_error_fails_the_boot_naming_the_key() {
    let config = R2eConfig::from_yaml_str("engines:\n  - kind: local\n    key: k\n").unwrap();
    let err = AppBuilder::new()
        .override_config(config)
        .load_config::<SerdeConfig<RootSettings>>()
        .try_build_state()
        .await
        .err()
        .expect("a missing serde field must fail the boot");
    let rendered = err.to_string();
    assert!(rendered.contains("missing field `name`"), "{rendered}");
}

// ── Nested inside a derived ConfigProperties ───────────────────────────────

#[derive(Deserialize, Clone, Debug, PartialEq)]
struct EngineSettings {
    key: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct NativeRoot {
    name: String,
    #[config(section)]
    engine: SerdeConfig<EngineSettings>,
}

#[test]
fn serde_island_inside_a_derived_section() {
    let config =
        R2eConfig::from_yaml_str("app:\n  name: n\n  engine:\n    key: k\n    tags: [x]\n")
            .unwrap();
    let root = NativeRoot::from_config(&config, Some("app")).unwrap();
    assert_eq!(root.name, "n");
    assert_eq!(
        root.engine.0,
        EngineSettings {
            key: "k".into(),
            tags: vec!["x".into()],
        }
    );
}

#[r2e_core::test]
async fn serde_island_is_injectable_from_a_derived_root() {
    let config = R2eConfig::from_yaml_str("name: n\nengine:\n  key: k\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<NativeRoot>()
        .try_build_state()
        .await
        .expect("derived root with serde island binds");
    assert_eq!(app.state().get::<EngineSettings>().key, "k");
    assert_eq!(app.state().get::<SerdeConfig<EngineSettings>>().key, "k");
}

// ── Plugin Config ──────────────────────────────────────────────────────────

struct SerdePlugin {
    sink: Arc<Mutex<Option<Option<EngineSettings>>>>,
}

impl Plugin for SerdePlugin {
    type Provided = ();
    type Deps = ();
    type Config = SerdeConfig<EngineSettings>;
    type Controllers = ();
    const CONFIG_PREFIX: Option<&'static str> = Some("engine");

    async fn build(
        self,
        _deps: (),
        config: Option<SerdeConfig<EngineSettings>>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(), PluginBuildError> {
        *self.sink.lock().unwrap() = Some(config.map(SerdeConfig::into_inner));
        Ok(())
    }
}

#[r2e_core::test]
async fn plugin_config_can_be_a_serde_config() {
    let sink = Arc::new(Mutex::new(None));
    let config = R2eConfig::from_yaml_str("engine:\n  key: plug\n").unwrap();
    let _app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(SerdePlugin { sink: sink.clone() })
        .build_state()
        .await;

    let received = sink.lock().unwrap().clone().expect("build ran");
    assert_eq!(
        received,
        Some(EngineSettings {
            key: "plug".into(),
            tags: vec![],
        })
    );
}
