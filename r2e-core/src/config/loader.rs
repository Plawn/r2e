use std::collections::HashMap;
use std::path::Path;

use super::value::ConfigValue;
use super::ConfigError;

/// One step of the YAML path that produced a flat key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Segment {
    /// The mapping key as written, or the decimal sequence index.
    pub(crate) text: String,
    /// `true` for a mapping key, `false` for a sequence index.
    pub(crate) key: bool,
}

/// The YAML paths of the flat keys that do **not** survive `split('.')`:
/// a mapping key that contains a dot (`"gpt-4.1"`) or that reads as a list
/// index (`"0"`). Every other flat key has no entry — its path is its dotted
/// form, which is also the only reading available for keys written by the
/// `R2E_` overlay, a provider or `R2eConfig::set`.
pub(crate) type KeyPaths = HashMap<String, Vec<Segment>>;

/// The flat form of the YAML loaded so far: values by dotted key, plus the
/// original paths [`tree`](super::tree) needs to rebuild them losslessly.
#[derive(Default)]
pub(crate) struct Flat {
    pub(crate) values: HashMap<String, ConfigValue>,
    pub(crate) paths: KeyPaths,
}

impl Flat {
    fn insert(&mut self, key: &str, path: &[Segment], value: ConfigValue) {
        self.values.insert(key.to_string(), value);
        let ambiguous = path.iter().any(|segment| {
            segment.key && (segment.text.contains('.') || super::tree::is_index(&segment.text))
        });
        if ambiguous {
            self.paths.insert(key.to_string(), path.to_vec());
        } else {
            // A profile file may redefine the key through an unambiguous path.
            self.paths.remove(key);
        }
    }
}

/// Load and parse a YAML file, flattening it into `out`.
pub(crate) fn load_yaml_file(path: &Path, out: &mut Flat) -> Result<(), ConfigError> {
    if path.exists() {
        let content =
            std::fs::read_to_string(path).map_err(|e| ConfigError::Load(e.to_string()))?;
        load_yaml_str(&content, out)?;
    }
    Ok(())
}

/// Parse a YAML string and flatten it into `out`.
pub(crate) fn load_yaml_str(content: &str, out: &mut Flat) -> Result<(), ConfigError> {
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(content).map_err(|e| ConfigError::Load(e.to_string()))?;
    flatten_yaml("", &mut Vec::new(), &yaml, out);
    Ok(())
}

/// The text of a YAML mapping key. Scalars keep their YAML spelling (`200`,
/// `true`), so an unquoted numeric key reads like the quoted one.
pub(crate) fn mapping_key(key: &serde_yaml::Value) -> String {
    match key {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        other => format!("{other:?}"),
    }
}

/// Flatten a YAML tree into dot-separated keys.
///
/// A sequence is stored whole under its parent key **and** element by element
/// (`key.0`, `key.1`, …) for env var compat. An empty mapping is stored as an
/// empty `Map` at its own key, so it does not vanish from the rebuilt tree.
fn flatten_yaml(prefix: &str, path: &mut Vec<Segment>, value: &serde_yaml::Value, out: &mut Flat) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            if map.is_empty() && !prefix.is_empty() {
                out.insert(prefix, path, ConfigValue::Map(HashMap::new()));
            }
            for (k, v) in map {
                flatten_child(prefix, path, mapping_key(k), true, v, out);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            if !prefix.is_empty() {
                out.insert(
                    prefix,
                    path,
                    ConfigValue::List(seq.iter().map(ConfigValue::from_yaml).collect()),
                );
                for (i, item) in seq.iter().enumerate() {
                    flatten_child(prefix, path, i.to_string(), false, item, out);
                }
            }
        }
        leaf => {
            if !prefix.is_empty() {
                out.insert(prefix, path, ConfigValue::from_yaml(leaf));
            }
        }
    }
}

fn flatten_child(
    prefix: &str,
    path: &mut Vec<Segment>,
    text: String,
    key: bool,
    value: &serde_yaml::Value,
    out: &mut Flat,
) {
    let full_key = if prefix.is_empty() {
        text.clone()
    } else {
        format!("{prefix}.{text}")
    };
    path.push(Segment { text, key });
    flatten_yaml(&full_key, path, value, out);
    path.pop();
}
