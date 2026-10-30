//! The `ConfigValue` serde deserializer behind `deserialize_value` and
//! `SerdeConfig<T>`: string coercion on typed hints, `deserialize_any`
//! passthrough, option/null handling, enums.

use std::collections::HashMap;

use r2e_core::config::{
    deserialize_value, ConfigError, ConfigProperties, ConfigValue, R2eConfig, SerdeConfig,
};
use serde::Deserialize;

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

fn de<T: serde::de::DeserializeOwned>(value: ConfigValue) -> Result<T, String> {
    deserialize_value::<T>(&value, "k").map_err(|e| match e {
        ConfigError::Deserialize { key, message } => {
            assert_eq!(key, "k", "deserialize_value keeps the caller's key");
            message
        }
        other => panic!("unexpected error {other:?}"),
    })
}

// ── Coercion matrix ────────────────────────────────────────────────────────

#[derive(Deserialize, Debug, PartialEq)]
struct Typed {
    port: u16,
    offset: i32,
    big: u64,
    ratio: f64,
    small: f32,
    enabled: bool,
    letter: char,
}

#[test]
fn strings_coerce_on_typed_hints() {
    let typed: Typed = de(map(&[
        ("port", s("8080")),
        ("offset", s("-3")),
        ("big", s("18446744073709551615")),
        ("ratio", s("0.25")),
        ("small", s("1.5")),
        ("enabled", s("yes")),
        ("letter", s("x")),
    ]))
    .unwrap();
    assert_eq!(
        typed,
        Typed {
            port: 8080,
            offset: -3,
            big: u64::MAX,
            ratio: 0.25,
            small: 1.5,
            enabled: true,
            letter: 'x',
        }
    );
}

#[test]
fn native_values_still_bind() {
    let typed: Typed = de(map(&[
        ("port", ConfigValue::Integer(80)),
        ("offset", ConfigValue::Integer(-1)),
        ("big", ConfigValue::Integer(7)),
        ("ratio", ConfigValue::Integer(2)),
        ("small", ConfigValue::Float(0.5)),
        ("enabled", ConfigValue::Bool(false)),
        ("letter", s("y")),
    ]))
    .unwrap();
    assert_eq!(typed.port, 80);
    assert_eq!(typed.ratio, 2.0);
    assert!(!typed.enabled);
}

#[test]
fn bool_coercion_matches_from_config_value() {
    for (raw, expected) in [
        ("true", true),
        ("TRUE", true),
        ("1", true),
        ("Yes", true),
        ("false", false),
        ("0", false),
        ("NO", false),
    ] {
        assert_eq!(de::<bool>(s(raw)).unwrap(), expected, "{raw}");
    }
    let err = de::<bool>(s("maybe")).unwrap_err();
    assert!(err.contains("invalid type"), "{err}");
}

#[test]
fn integer_range_and_parse_errors() {
    let err = de::<u16>(s("70000")).unwrap_err();
    assert!(err.contains("70000"), "{err}");
    let err = de::<u16>(s("-1")).unwrap_err();
    assert!(err.contains("-1"), "{err}");
    let err = de::<i64>(s("abc")).unwrap_err();
    assert!(err.contains("invalid type: string \"abc\""), "{err}");
}

#[test]
fn scalars_format_into_strings() {
    assert_eq!(de::<String>(ConfigValue::Integer(42)).unwrap(), "42");
    assert_eq!(de::<String>(ConfigValue::Float(1.5)).unwrap(), "1.5");
    assert_eq!(de::<String>(ConfigValue::Bool(true)).unwrap(), "true");
    assert!(de::<String>(ConfigValue::Null).is_err());
}

#[test]
fn single_scalar_is_a_one_element_seq() {
    assert_eq!(de::<Vec<u16>>(s("5")).unwrap(), vec![5]);
    assert_eq!(
        de::<Vec<String>>(ConfigValue::List(vec![s("a"), ConfigValue::Integer(1)])).unwrap(),
        vec!["a".to_string(), "1".to_string()]
    );
    assert_eq!(
        de::<Vec<u8>>(ConfigValue::List(vec![s("1"), s("2")])).unwrap(),
        vec![1, 2]
    );
}

#[test]
fn tuple_rejects_extra_elements() {
    let three = ConfigValue::List(vec![
        ConfigValue::Integer(1),
        ConfigValue::Integer(2),
        ConfigValue::Integer(3),
    ]);
    assert!(de::<(u8, u8)>(three).is_err());
}

#[test]
fn map_values_coerce_and_keys_stay_strings() {
    let parsed: HashMap<String, u32> =
        de(map(&[("a", s("1")), ("b", ConfigValue::Integer(2))])).unwrap();
    assert_eq!(parsed["a"], 1);
    assert_eq!(parsed["b"], 2);
}

// ── deserialize_any passthrough ────────────────────────────────────────────

#[derive(Deserialize, Debug, PartialEq)]
#[serde(untagged)]
enum PortOrName {
    Port(u16),
    Name(String),
}

#[test]
fn untagged_enum_sees_the_value_as_stored() {
    assert_eq!(
        de::<PortOrName>(ConfigValue::Integer(8080)).unwrap(),
        PortOrName::Port(8080)
    );
    // No coercion below `deserialize_any`: a string stays a string.
    assert_eq!(
        de::<PortOrName>(s("8080")).unwrap(),
        PortOrName::Name("8080".into())
    );
}

#[test]
fn json_value_field_is_not_coerced() {
    #[derive(Deserialize)]
    struct Holder {
        raw: serde_json::Value,
    }
    let holder: Holder = de(map(&[(
        "raw",
        map(&[
            ("port", s("8080")),
            ("tags", ConfigValue::List(vec![s("a")])),
        ]),
    )]))
    .unwrap();
    assert_eq!(
        holder.raw,
        serde_json::json!({ "port": "8080", "tags": ["a"] })
    );
}

// ── Option / Null ──────────────────────────────────────────────────────────

#[derive(Deserialize, Debug, Default, PartialEq)]
struct Tls {
    #[serde(default)]
    cert: Option<String>,
    #[serde(default)]
    ports: Vec<u16>,
}

#[test]
fn option_and_null() {
    assert_eq!(de::<Option<u16>>(ConfigValue::Null).unwrap(), None);
    assert_eq!(de::<Option<u16>>(s("7")).unwrap(), Some(7));
    assert_eq!(de::<Option<Tls>>(ConfigValue::Null).unwrap(), None);
    // Struct / map / seq hints read `Null` as empty.
    assert_eq!(de::<Tls>(ConfigValue::Null).unwrap(), Tls::default());
    assert!(de::<HashMap<String, String>>(ConfigValue::Null)
        .unwrap()
        .is_empty());
    assert!(de::<Vec<u16>>(ConfigValue::Null).unwrap().is_empty());
}

// ── Enums ──────────────────────────────────────────────────────────────────

#[derive(Deserialize, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Shape {
    Point,
    Circle(f64),
    Square { side: u32 },
    Pair(u8, u8),
}

#[test]
fn externally_tagged_enums_in_a_vec_coerce_their_fields() {
    let shapes: Vec<Shape> = de(ConfigValue::List(vec![
        s("point"),
        map(&[("circle", s("2.5"))]),
        map(&[("square", map(&[("side", s("3"))]))]),
        map(&[(
            "pair",
            ConfigValue::List(vec![s("1"), ConfigValue::Integer(2)]),
        )]),
    ]))
    .unwrap();
    assert_eq!(
        shapes,
        vec![
            Shape::Point,
            Shape::Circle(2.5),
            Shape::Square { side: 3 },
            Shape::Pair(1, 2),
        ]
    );
    assert!(de::<Shape>(s("hexagon")).is_err());
    assert!(de::<Shape>(ConfigValue::Integer(1)).is_err());
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Engine {
    Local { key: String },
    Remote { key: String, port: u16 },
}

#[test]
fn internally_tagged_enum_in_a_vec() {
    let engines: Vec<Engine> = de(ConfigValue::List(vec![
        map(&[("kind", s("local")), ("key", s("l"))]),
        map(&[
            ("kind", s("remote")),
            ("key", s("r")),
            ("port", ConfigValue::Integer(9000)),
        ]),
    ]))
    .unwrap();
    assert_eq!(
        engines,
        vec![
            Engine::Local { key: "l".into() },
            Engine::Remote {
                key: "r".into(),
                port: 9000
            },
        ]
    );
}

// ── End to end through SerdeConfig ─────────────────────────────────────────

#[derive(Deserialize, Clone, Debug)]
struct Server {
    port: u16,
    debug: bool,
    #[serde(default)]
    hosts: Vec<String>,
}

#[test]
fn r2e_env_override_binds_non_string_fields() {
    let _env = crate::support::env_lock();
    crate::support::set_env("R2E_SERVER_PORT", "9090");
    crate::support::set_env("R2E_SERVER_DEBUG", "true");

    let mut config =
        R2eConfig::from_yaml_str("server:\n  port: 3000\n  debug: false\n  hosts: [a]\n").unwrap();
    config.apply_current_env_overlay();

    crate::support::remove_env("R2E_SERVER_PORT");
    crate::support::remove_env("R2E_SERVER_DEBUG");

    let server = SerdeConfig::<Server>::from_config(&config, Some("server")).unwrap();
    assert_eq!(server.port, 9090);
    assert!(server.debug);
    assert_eq!(server.hosts, vec!["a".to_string()]);
}

#[test]
fn coercion_error_names_the_key() {
    let config = R2eConfig::from_yaml_str("server:\n  port: \"http\"\n  debug: true\n").unwrap();
    match SerdeConfig::<Server>::from_config(&config, Some("server")) {
        Err(ConfigError::Deserialize { key, message }) => {
            assert_eq!(key, "server.port");
            assert!(message.contains("expected u16"), "{message}");
        }
        other => panic!("expected a Deserialize error, got {other:?}"),
    }
}
