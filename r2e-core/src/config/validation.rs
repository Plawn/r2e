use super::typed::ConfigProperties;
use super::{ConfigError, R2eConfig};

/// Derive the `R2E_` env-var hint for a config key — but only when the key is
/// actually reachable through the env overlay.
///
/// The overlay mapping is strict: `R2E_X_Y` → `x.y` (every `_` becomes `.`,
/// nothing else). It can therefore never produce a `-` nor an in-segment `_`:
/// a kebab-case key (`database.min-idle`) or snake_case key
/// (`database.max_idle`) is **not** addressable via any `R2E_` var
/// (`R2E_DATABASE_MAX_IDLE` would insert `database.max.idle`), and this
/// returns `None` so callers fall back to YAML/placeholder wording. Purely
/// dotted keys return their full working var — `R2E_` prefix included, since
/// unprefixed env vars are ignored by the overlay.
pub(crate) fn derived_env_hint(key: &str) -> Option<String> {
    if key.contains('-') || key.contains('_') {
        None
    } else {
        Some(format!("R2E_{}", key.to_uppercase().replace('.', "_")))
    }
}

/// A single missing config key.
#[derive(Debug)]
pub struct MissingKeyError {
    /// Source that requires this key (bean name, controller name, section prefix).
    pub source: String,
    /// The config key that is missing.
    pub key: String,
    /// The expected type name.
    pub expected_type: String,
    /// Environment variable hint: the `R2E_…` var (or exact `#[config(env)]`
    /// var) that sets this key. `None` when the key is not env-reachable
    /// (contains `-` or `_`): settable via YAML / `${VAR}` placeholder only.
    pub env_hint: Option<String>,
    /// Optional description (from `ConfigProperties` metadata).
    pub description: Option<String>,
}

impl std::fmt::Display for MissingKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "  - `{}`: key '{}' ({})",
            self.source, self.key, self.expected_type
        )?;
        match &self.env_hint {
            Some(hint) => write!(f, " — set env var `{}`", hint)?,
            None => write!(
                f,
                " — set '{}' in application.yaml (keys containing '-' or '_' are not addressable via R2E_ env vars; use a ${{VAR}} placeholder for env-driven values)",
                self.key
            )?,
        }
        if let Some(desc) = &self.description {
            write!(f, " -- {}", desc)?;
        }
        Ok(())
    }
}

/// Aggregated config validation error.
#[derive(Debug)]
pub struct ConfigValidationError {
    pub errors: Vec<MissingKeyError>,
}

impl std::fmt::Display for ConfigValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Missing configuration keys:")?;
        for err in &self.errors {
            writeln!(f, "{}", err)?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigValidationError {}

/// Validate a list of config keys against an `R2eConfig`.
///
/// Each entry is `(source_name, key, type_name)`. Returns the list of
/// missing keys as [`MissingKeyError`]s (empty if all present).
pub fn validate_keys(config: &R2eConfig, keys: &[(&str, &str, &str)]) -> Vec<MissingKeyError> {
    keys.iter()
        .filter(|(_, key, _)| !config.contains_key(key))
        .map(|(source, key, type_name)| MissingKeyError {
            source: source.to_string(),
            key: key.to_string(),
            expected_type: type_name.to_string(),
            env_hint: derived_env_hint(key),
            description: None,
        })
        .collect()
}

/// Presence-validate a host's **declared** `config_keys()` list.
///
/// The single shared bridge from a `(key, type name, kind)` declaration to
/// [`validate_keys`]: only [`ConfigKeyKind::Required`] entries are checked —
/// `Optional` resolves to `None` when absent, `Section` validates itself at
/// construction, and `Live` legitimately starts empty (see the
/// [`ConfigKeyKind`] docs).
///
/// Used by the hosts whose declarations are *not* bean registrations and
/// therefore never reach `BeanRegistry::validate_all_config`: decorator beans
/// (through [`DecoratorSpec::config_keys`](crate::decorators::decorator::DecoratorSpec::config_keys),
/// aggregated into `Controller::validate_config`) and background services
/// (through [`ServiceComponent::config_keys`](crate::ServiceComponent::config_keys),
/// aggregated at `spawn_service` / at graph resolution for `#[producer(start)]`).
///
/// `Section` entries carry only a prefix and a type *name*, which is not enough
/// to walk the section's required keys — those hosts declare their sections a
/// second time, as [`SectionValidator`]s, and validate them through
/// [`validate_declared_sections`].
pub fn validate_declared_keys(
    source: &str,
    declared: &[(&'static str, &'static str, super::ConfigKeyKind)],
    config: &R2eConfig,
) -> Vec<MissingKeyError> {
    let required: Vec<(&str, &str, &str)> = declared
        .iter()
        .filter(|(_, _, kind)| kind.is_required())
        .map(|(key, ty_name, _)| (source, *key, *ty_name))
        .collect();
    validate_keys(config, &required)
}

/// A **type-aware** `#[config_section]` declaration: a prefix bound to the
/// [`validate_section`] instantiation for the section's `ConfigProperties`
/// type.
///
/// A `config_keys()` entry cannot express this: it carries the prefix and the
/// type's *name*, so the bridge that reads it has no way back to the type and
/// can only skip `Section` entries (their kind is not
/// [`is_required`](super::ConfigKeyKind::is_required)). That is fine for beans
/// and controllers — a bean constructs during `build_state` and a controller's
/// generated `__r2e_meta::validate_config` calls `validate_section::<Ty>`
/// directly, both at startup. It is *not* fine for hosts built later: a
/// decorator bean (built in `build_decorator`, at registration) and a
/// background service (built in `ServiceComponent::from_context`, when the task
/// starts) would otherwise only discover a missing section key by panicking.
///
/// So those two derives emit their sections **twice**: once in `config_keys()`
/// as a `Section` entry (which is what a host bean fingerprints for dev-reload)
/// and once here as a validator the host can actually run. Validation is the
/// full [`validate_section`] walk — missing required leaves, nested sections,
/// type mismatches and `garde` violations alike, exactly what a controller
/// field gets.
#[derive(Clone, Copy)]
pub struct SectionValidator {
    prefix: &'static str,
    validate: fn(&R2eConfig, Option<&str>) -> Vec<MissingKeyError>,
}

impl SectionValidator {
    /// Declare `prefix` as a section of type `C`.
    #[must_use]
    pub fn of<C: ConfigProperties>(prefix: &'static str) -> Self {
        Self {
            prefix,
            validate: validate_section::<C>,
        }
    }

    /// The declared section prefix.
    #[must_use]
    pub fn prefix(&self) -> &'static str {
        self.prefix
    }

    /// Run the section's own validation against `config`.
    #[must_use]
    pub fn validate(&self, config: &R2eConfig) -> Vec<MissingKeyError> {
        (self.validate)(config, Some(self.prefix))
    }
}

impl std::fmt::Debug for SectionValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SectionValidator")
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

/// Run every declared [`SectionValidator`], flattening the reports.
///
/// The section companion of [`validate_declared_keys`] — same hosts, same
/// aggregation points.
pub fn validate_declared_sections(
    sections: &[SectionValidator],
    config: &R2eConfig,
) -> Vec<MissingKeyError> {
    sections
        .iter()
        .flat_map(|section| section.validate(config))
        .collect()
}

/// Phase-1 missing-key walk for a `ConfigProperties` section.
///
/// Reports every missing required key — the section's own leaves plus, via
/// the derive-generated `PropertyMeta::validate_nested` hooks, every nested
/// `#[config(section)]`'s keys — with full per-property metadata. This walk
/// is metadata-only: it never calls `from_config`, so recursion does not
/// construct sections (no repeated secret resolution / deserialization).
/// [`validate_section`] adds the single top-level construction probe.
pub fn validate_section_keys<C: ConfigProperties>(
    config: &R2eConfig,
    prefix: Option<&str>,
) -> Vec<MissingKeyError> {
    let meta = C::properties_metadata(prefix);
    let source = prefix.unwrap_or("<root>");

    let mut errors: Vec<MissingKeyError> = meta
        .iter()
        .filter(|prop| prop.required && !prop.is_section)
        // The probe is generated by the derive next to `from_config` and is
        // the single owner of resolution semantics (config map, custom
        // `#[config(env = "VAR")]` var, any future source).
        .filter(|prop| !prop.is_resolvable(config))
        .map(|prop| MissingKeyError {
            source: source.to_string(),
            key: prop.full_key.clone(),
            expected_type: prop.type_name.to_string(),
            env_hint: match &prop.env_var {
                Some(env) => Some(env.clone()),
                None => derived_env_hint(&prop.full_key),
            },
            description: prop.description.clone(),
        })
        .collect();

    // Recurse into nested `#[config(section)]` properties. The hook is
    // generated by the derive next to `from_config` and owns the presence
    // semantics (mandatory → always, optional/defaulted → only when present,
    // map → per entry), so a nested section's missing required keys are all
    // reported here in phase 1 with full metadata instead of dying on the
    // phase-2 probe's first `NotFound`. `validate_nested` is `Some` only on
    // section properties.
    for prop in &meta {
        if let Some(validate) = prop.validate_nested {
            errors.extend(validate(config, prop));
        }
    }

    errors
}

/// Validate a `ConfigProperties` section against an `R2eConfig`.
///
/// Checks that all required keys are present (including nested
/// `#[config(section)]` keys, via [`validate_section_keys`]). Also attempts
/// to construct the section via `from_config` to detect type mismatches and
/// validation errors (e.g., garde constraints).
pub fn validate_section<C: ConfigProperties>(
    config: &R2eConfig,
    prefix: Option<&str>,
) -> Vec<MissingKeyError> {
    let source = prefix.unwrap_or("<root>");
    let mut errors = validate_section_keys::<C>(config, prefix);

    // If no missing keys, try constructing the section to surface
    // TypeMismatch and Validation errors. This is also the only reporting
    // path for manual `ConfigProperties` impls that keep the default (empty)
    // `properties_metadata` — the `NotFound` arm below stays live for them.
    if errors.is_empty() {
        if let Err(e) = C::from_config(config, prefix) {
            match e {
                ConfigError::TypeMismatch { key, expected } => {
                    errors.push(MissingKeyError {
                        source: source.to_string(),
                        key: key.clone(),
                        expected_type: expected.to_string(),
                        env_hint: derived_env_hint(&key),
                        description: Some(format!("type mismatch: expected {expected}")),
                    });
                }
                ConfigError::Validation(details) => {
                    for detail in details {
                        errors.push(MissingKeyError {
                            source: source.to_string(),
                            key: detail.key.clone(),
                            expected_type: "valid".to_string(),
                            env_hint: derived_env_hint(&detail.key),
                            description: Some(detail.message),
                        });
                    }
                }
                ConfigError::NotFound(key) => {
                    errors.push(MissingKeyError {
                        source: source.to_string(),
                        key: key.clone(),
                        expected_type: "unknown".to_string(),
                        env_hint: derived_env_hint(&key),
                        description: None,
                    });
                }
                ConfigError::Deserialize { key, message } => {
                    errors.push(MissingKeyError {
                        source: source.to_string(),
                        key: key.clone(),
                        expected_type: "deserializable".to_string(),
                        env_hint: derived_env_hint(&key),
                        description: Some(message),
                    });
                }
                ConfigError::Load(msg) => {
                    errors.push(MissingKeyError {
                        source: source.to_string(),
                        key: source.to_string(),
                        expected_type: "loadable".to_string(),
                        env_hint: derived_env_hint(source),
                        description: Some(msg),
                    });
                }
            }
        }
    }

    errors
}
