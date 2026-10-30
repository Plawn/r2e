//! [`SerdeConfig<T>`]: load any `serde::Deserialize` type as a typed config
//! section, without `#[derive(ConfigProperties)]`.

use std::ops::Deref;

use serde::de::DeserializeOwned;

use super::de::ValueDeserializer;
use super::typed::ConfigProperties;
use super::value::ConfigValue;
use super::{ConfigError, R2eConfig};
use crate::type_list::{TCons, TNil};

/// A typed config section backed by **serde** instead of
/// `#[derive(ConfigProperties)]`.
///
/// Wrap any `T: DeserializeOwned + Clone` — typically a settings struct that
/// lives in a framework-free crate — and it becomes a full
/// [`ConfigProperties`] type: `load_config::<SerdeConfig<T>>()`,
/// `provide_config`, a plugin's `type Config`, a `#[config(section)]` field.
/// `T` itself is registered as a bean next to the wrapper, so beans and
/// controllers `#[inject] cfg: T` directly.
///
/// The section is rebuilt with [`R2eConfig::tree`] (flat keys are the source
/// of truth, so `R2E_*` overrides and `override_config_value` on indexed keys
/// such as `engines.0.key` are visible) and deserialized with serde, so every
/// serde attribute works (`untagged`, `default = "fn"`, `rename_all`, enums in
/// `Vec<struct>`, …). An absent section deserializes from an empty map.
///
/// Strings are coerced on typed fields with the same rules as
/// [`FromConfigValue`](super::FromConfigValue) — `R2E_APP_PORT=8080` binds to
/// a `u16`, `"yes"` to a `bool` — because every env override and resolved
/// `${VAR}` is stored as a string. Coercion does not apply below
/// `deserialize_any` (untagged / internally tagged enums, `#[serde(flatten)]`):
/// serde buffers those values as stored.
///
/// ```ignore
/// #[derive(serde::Deserialize, Clone, Debug)]
/// pub struct AppConfig {
///     pub name: String,
///     #[serde(default)]
///     pub engines: Vec<EngineConfig>,
/// }
///
/// AppBuilder::new()
///     .load_config::<SerdeConfig<AppConfig>>()   // provides SerdeConfig<AppConfig> AND AppConfig
///     // …
/// ```
///
/// Errors are reported as [`ConfigError::Deserialize`] whose `key` is the r2e
/// key of the offending value (`app.engines.1.weight`); a `missing field`
/// error carries the parent path as `key` and names the field in `message`.
///
/// Trade-offs compared to the derive: serde stops at the first error, so the
/// boot report lists one problem at a time instead of every missing key; and
/// there is no [`PropertyMeta`](super::PropertyMeta), so introspection
/// (`r2e doctor`, docs listing) cannot enumerate the section's fields.
#[derive(Clone, Debug)]
pub struct SerdeConfig<T>(pub T);

impl<T> SerdeConfig<T> {
    /// Unwrap the deserialized value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for SerdeConfig<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> ConfigProperties for SerdeConfig<T>
where
    T: DeserializeOwned + Clone + Send + Sync + 'static,
{
    type Children = TCons<T, TNil>;

    fn from_config(config: &R2eConfig, prefix: Option<&str>) -> Result<Self, ConfigError> {
        let tree = match config.tree(prefix) {
            None | Some(ConfigValue::Null) => ConfigValue::Map(Default::default()),
            Some(tree) => tree,
        };
        deserialize_tree(&tree, prefix).map(SerdeConfig)
    }

    fn register_children(&self, registry: &mut crate::beans::BeanRegistry) {
        registry.provide(self.0.clone());
    }
}

/// Deserialize `T` from a tree rebuilt under `prefix`, naming the offending
/// r2e key on error.
fn deserialize_tree<T: DeserializeOwned>(
    tree: &ConfigValue,
    prefix: Option<&str>,
) -> Result<T, ConfigError> {
    serde_path_to_error::deserialize(ValueDeserializer(tree)).map_err(|err| {
        ConfigError::Deserialize {
            key: error_key(err.path(), prefix),
            message: err.into_inner().to_string(),
        }
    })
}

/// Map a serde path (`engines[1].backends[0].weight`) to the r2e key
/// (`<prefix>.engines.1.backends.0.weight`).
fn error_key(path: &serde_path_to_error::Path, prefix: Option<&str>) -> String {
    use serde_path_to_error::Segment;

    let segments: Vec<String> = prefix
        .map(str::to_string)
        .into_iter()
        .chain(path.iter().filter_map(|segment| match segment {
            Segment::Seq { index } => Some(index.to_string()),
            Segment::Map { key } => Some(key.clone()),
            Segment::Enum { variant } => Some(variant.clone()),
            Segment::Unknown => None,
        }))
        .collect();
    if segments.is_empty() {
        "<root>".to_string()
    } else {
        segments.join(".")
    }
}
