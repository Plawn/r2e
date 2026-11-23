//! `R2eConfig::tree`: rebuilding a nested `ConfigValue` from the flat keys.

use std::collections::HashMap;

use r2e_core::config::{ConfigValue, R2eConfig};

fn s(v: &str) -> ConfigValue {
    ConfigValue::String(v.to_string())
}

fn map(entries: &[(&str, ConfigValue)]) -> ConfigValue {
    ConfigValue::Map(
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

/// `ConfigValue` has no `PartialEq` (it holds `f64`); compare through a
/// canonical rendering with sorted map keys.
fn render(v: &ConfigValue) -> String {
    match v {
        ConfigValue::String(s) => format!("{s:?}"),
        ConfigValue::Integer(i) => i.to_string(),
        ConfigValue::Float(f) => format!("{f:?}"),
        ConfigValue::Bool(b) => b.to_string(),
        ConfigValue::Null => "null".to_string(),
        ConfigValue::List(items) => {
            let items: Vec<String> = items.iter().map(render).collect();
            format!("[{}]", items.join(", "))
        }
        ConfigValue::Map(m) => {
            let mut entries: Vec<(&String, &ConfigValue)> = m.iter().collect();
            entries.sort_by_key(|(k, _)| *k);
            let entries: Vec<String> = entries
                .into_iter()
                .map(|(k, v)| format!("{k}: {}", render(v)))
                .collect();
            format!("{{{}}}", entries.join(", "))
        }
    }
}

fn assert_tree(actual: Option<ConfigValue>, expected: ConfigValue) {
    let actual = actual.expect("tree must be present");
    assert_eq!(render(&actual), render(&expected));
}

#[test]
fn yaml_map_list_and_scalars() {
    let config = R2eConfig::from_yaml_str(
        r#"
app:
  name: demo
  port: 8080
  ratio: 0.5
  debug: true
  tags: [a, b]
  db:
    url: postgres://x
"#,
    )
    .unwrap();

    assert_tree(
        config.tree(Some("app")),
        map(&[
            ("name", s("demo")),
            ("port", ConfigValue::Integer(8080)),
            ("ratio", ConfigValue::Float(0.5)),
            ("debug", ConfigValue::Bool(true)),
            ("tags", ConfigValue::List(vec![s("a"), s("b")])),
            ("db", map(&[("url", s("postgres://x"))])),
        ]),
    );
    assert_tree(config.tree(Some("app.name")), s("demo"));
    assert_tree(
        config.tree(Some("app.db")),
        map(&[("url", s("postgres://x"))]),
    );
}

#[test]
fn absent_prefix_is_none() {
    let config = R2eConfig::from_yaml_str("app:\n  name: demo\n").unwrap();
    assert!(config.tree(Some("other")).is_none());
    assert!(
        config.tree(Some("app.nam")).is_none(),
        "no partial-segment match"
    );
    assert!(R2eConfig::empty().tree(None).is_none());
}

#[test]
fn empty_yaml_list_stays_a_list() {
    let config = R2eConfig::from_yaml_str("app:\n  tags: []\n").unwrap();
    assert_tree(config.tree(Some("app.tags")), ConfigValue::List(vec![]));
    assert_tree(
        config.tree(Some("app")),
        map(&[("tags", ConfigValue::List(vec![]))]),
    );
}

#[test]
fn list_of_structs_keeps_empty_sub_maps_from_the_seed() {
    let config = R2eConfig::from_yaml_str(
        r#"
engines:
  - key: a
    extra: {}
  - key: b
"#,
    )
    .unwrap();
    assert_tree(
        config.tree(Some("engines")),
        ConfigValue::List(vec![
            map(&[("key", s("a")), ("extra", map(&[]))]),
            map(&[("key", s("b"))]),
        ]),
    );
}

#[test]
fn indexed_override_wins_over_parent_list() {
    let mut config = R2eConfig::from_yaml_str(
        r#"
engines:
  - key: a
    backends:
      - weight: 1
  - key: b
"#,
    )
    .unwrap();
    // What `override_config_value` / the `R2E_` overlay write: flat keys only.
    config.set("engines.0.key", s("overridden"));
    config.set("engines.0.backends.0.weight", s("7"));

    assert_tree(
        config.tree(Some("engines")),
        ConfigValue::List(vec![
            map(&[
                ("key", s("overridden")),
                (
                    "backends",
                    ConfigValue::List(vec![map(&[("weight", s("7"))])]),
                ),
            ]),
            map(&[("key", s("b"))]),
        ]),
    );
}

#[test]
fn env_overlay_override_is_visible_in_list() {
    // The flattened form of `engines: [{key: a}]`.
    let mut values = HashMap::new();
    values.insert(
        "engines".to_string(),
        ConfigValue::List(vec![map(&[("key", s("a"))])]),
    );
    values.insert("engines.0.key".to_string(), s("a"));
    R2eConfig::apply_env_overlay_for_test(
        &mut values,
        [("R2E_ENGINES_0_KEY".to_string(), "from-env".to_string())],
    );
    let mut config = R2eConfig::empty();
    for (k, v) in values {
        config.set(&k, v);
    }
    assert_tree(
        config.tree(Some("engines")),
        ConfigValue::List(vec![map(&[("key", s("from-env"))])]),
    );
}

#[test]
fn indexed_key_past_the_seed_extends_the_list() {
    let mut config = R2eConfig::from_yaml_str("tags: [a]\n").unwrap();
    config.set("tags.1", s("b"));
    assert_tree(
        config.tree(Some("tags")),
        ConfigValue::List(vec![s("a"), s("b")]),
    );
}

#[test]
fn env_only_list_from_dense_indices() {
    let mut config = R2eConfig::empty();
    config.set("hosts.1", s("b"));
    config.set("hosts.0", s("a"));
    config.set("hosts.2.name", s("c"));
    assert_tree(
        config.tree(Some("hosts")),
        ConfigValue::List(vec![s("a"), s("b"), map(&[("name", s("c"))])]),
    );
}

#[test]
fn sparse_or_non_canonical_indices_are_a_map() {
    let mut config = R2eConfig::empty();
    config.set("ids.1", s("b"));
    config.set("ids.3", s("d"));
    assert_tree(
        config.tree(Some("ids")),
        map(&[("1", s("b")), ("3", s("d"))]),
    );

    let mut config = R2eConfig::empty();
    config.set("ids.0", s("a"));
    config.set("ids.01", s("b"));
    assert_tree(
        config.tree(Some("ids")),
        map(&[("0", s("a")), ("01", s("b"))]),
    );
}

#[test]
fn scalar_with_children_drops_the_scalar() {
    let mut config = R2eConfig::empty();
    config.set("a", s("1"));
    config.set("a.b", s("2"));
    assert_tree(config.tree(Some("a")), map(&[("b", s("2"))]));
}

#[test]
fn null_section_is_some_null() {
    let config = R2eConfig::from_yaml_str("tls:\napp:\n  name: x\n").unwrap();
    assert_tree(config.tree(Some("tls")), ConfigValue::Null);

    // A null next to children: children win.
    let mut config = R2eConfig::from_yaml_str("tls:\n").unwrap();
    config.set("tls.cert", s("c.pem"));
    assert_tree(config.tree(Some("tls")), map(&[("cert", s("c.pem"))]));
}

#[test]
fn root_tree_covers_every_key() {
    let config = R2eConfig::from_yaml_str(
        r#"
name: demo
server:
  port: 3000
hosts: [a]
"#,
    )
    .unwrap();
    assert_tree(
        config.tree(None),
        map(&[
            ("name", s("demo")),
            ("server", map(&[("port", ConfigValue::Integer(3000))])),
            ("hosts", ConfigValue::List(vec![s("a")])),
        ]),
    );
}

// ── Lossless YAML paths (#1471) ────────────────────────────────────────────

#[test]
fn empty_yaml_table_stays_an_empty_map() {
    let config = R2eConfig::from_yaml_str("app:\n  name: x\n  limits: {}\n").unwrap();
    assert_tree(config.tree(Some("app.limits")), map(&[]));
    assert_tree(
        config.tree(Some("app")),
        map(&[("name", s("x")), ("limits", map(&[]))]),
    );
    // It still contributes no key: section presence is unchanged.
    assert!(!config.has_prefix("app.limits"));
    assert!(config.sub_keys("app.limits").is_empty());
}

#[test]
fn override_under_an_empty_table_fills_it() {
    let mut config = R2eConfig::from_yaml_str("limits: {}\n").unwrap();
    config.set("limits.rps", s("5"));
    assert_tree(config.tree(Some("limits")), map(&[("rps", s("5"))]));
}

#[test]
fn dotted_map_key_keeps_its_dots() {
    let config = R2eConfig::from_yaml_str(
        r#"
models:
  gpt-4.1:
    timeout: 30
  "claude.3.5": fast
  plain:
    timeout: 10
"#,
    )
    .unwrap();
    assert_tree(
        config.tree(Some("models")),
        map(&[
            ("gpt-4.1", map(&[("timeout", ConfigValue::Integer(30))])),
            ("claude.3.5", s("fast")),
            ("plain", map(&[("timeout", ConfigValue::Integer(10))])),
        ]),
    );
    // The root tree and the dotted prefix itself agree.
    assert_tree(
        config.tree(Some("models.gpt-4.1")),
        map(&[("timeout", ConfigValue::Integer(30))]),
    );
    assert_tree(
        config.tree(None),
        map(&[(
            "models",
            map(&[
                ("gpt-4.1", map(&[("timeout", ConfigValue::Integer(30))])),
                ("claude.3.5", s("fast")),
                ("plain", map(&[("timeout", ConfigValue::Integer(10))])),
            ]),
        )]),
    );
    // A prefix that stops inside the key is not a parent of it.
    assert!(config.tree(Some("models.gpt-4")).is_none());
    // Flat access is unchanged.
    assert_eq!(config.get::<i64>("models.gpt-4.1.timeout").unwrap(), 30);
}

#[test]
fn override_reaches_a_dotted_map_key() {
    let mut config = R2eConfig::from_yaml_str(
        "models:\n  gpt-4.1:\n    timeout: 30\n    name: four\n  gpt-4:\n    timeout: 5\n",
    )
    .unwrap();
    // Replaces an existing key, and adds a new one under the dotted key.
    config.set("models.gpt-4.1.timeout", s("60"));
    config.set("models.gpt-4.1.retries", s("2"));
    assert_tree(
        config.tree(Some("models")),
        map(&[
            (
                "gpt-4.1",
                map(&[
                    ("timeout", s("60")),
                    ("name", s("four")),
                    ("retries", s("2")),
                ]),
            ),
            ("gpt-4", map(&[("timeout", ConfigValue::Integer(5))])),
        ]),
    );
}

#[test]
fn digit_only_map_keys_stay_keys() {
    let config = R2eConfig::from_yaml_str(
        r#"
codes:
  "0": zero
  "1": one
statuses:
  200: ok
  404: missing
tags: [a, b]
"#,
    )
    .unwrap();
    assert_tree(
        config.tree(Some("codes")),
        map(&[("0", s("zero")), ("1", s("one"))]),
    );
    assert_tree(
        config.tree(Some("statuses")),
        map(&[("200", s("ok")), ("404", s("missing"))]),
    );
    // A real sequence is still a list.
    assert_tree(
        config.tree(Some("tags")),
        ConfigValue::List(vec![s("a"), s("b")]),
    );
}
