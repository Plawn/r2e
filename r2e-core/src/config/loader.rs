use std::collections::HashMap;
use std::path::Path;

use super::value::ConfigValue;
use super::ConfigError;

/// Load and parse a YAML file, flattening it into the values map.
pub(crate) fn load_yaml_file(
    path: &Path,
    values: &mut HashMap<String, ConfigValue>,
) -> Result<(), ConfigError> {
    if path.exists() {
        let content =
            std::fs::read_to_string(path).map_err(|e| ConfigError::Load(e.to_string()))?;
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&content).map_err(|e| ConfigError::Load(e.to_string()))?;
        flatten_yaml("", &yaml, values);
    }
    Ok(())
}

/// Parse a YAML string and flatten it into the values map.
pub(crate) fn load_yaml_str(
    content: &str,
    values: &mut HashMap<String, ConfigValue>,
) -> Result<(), ConfigError> {
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(content).map_err(|e| ConfigError::Load(e.to_string()))?;
    flatten_yaml("", &yaml, values);
    Ok(())
}

/// Flatten a YAML tree into dot-separated keys.
pub(crate) fn flatten_yaml(
    prefix: &str,
    value: &serde_yaml::Value,
    out: &mut HashMap<String, ConfigValue>,
) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                let key_str = match k {
                    serde_yaml::Value::String(s) => s.clone(),
                    other => format!("{other:?}"),
                };
                let full_key = if prefix.is_empty() {
                    key_str
                } else {
                    format!("{prefix}.{key_str}")
                };
                flatten_yaml(&full_key, v, out);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            if !prefix.is_empty() {
                // Store the full list under the parent key
                out.insert(
                    prefix.to_string(),
                    ConfigValue::List(seq.iter().map(ConfigValue::from_yaml).collect()),
                );
                // Also store each element individually (key.0, key.1, ...) for env var compat
                for (i, item) in seq.iter().enumerate() {
                    let indexed_key = format!("{prefix}.{i}");
                    flatten_yaml(&indexed_key, item, out);
                }
            }
        }
        leaf => {
            if !prefix.is_empty() {
                out.insert(prefix.to_string(), ConfigValue::from_yaml(leaf));
            }
        }
    }
}
