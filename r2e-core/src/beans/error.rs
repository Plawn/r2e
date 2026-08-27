use std::fmt;

/// Errors that can occur during bean graph resolution.
#[derive(Debug)]
pub enum BeanError {
    /// A dependency cycle was detected.
    CyclicDependency { cycle: Vec<String> },
    /// A bean declares a dependency that is neither registered nor provided.
    MissingDependency { bean: String, dependency: String },
    /// The same type was registered more than once.
    DuplicateBean { type_name: String },
    /// The same plugin was installed more than once — by the app and a
    /// module, or by two modules. A plugin has exactly one owner; every other
    /// module that needs it declares `requires_plugins(..)` instead.
    DuplicatePlugin {
        plugin: &'static str,
        /// Rendered owner labels in install order, e.g. `["app", "module 'Billing'"]`.
        owners: Vec<String>,
    },
    /// One or more config keys required by beans are missing.
    MissingConfigKeys(crate::config::ConfigValidationError),
    /// A post-construct hook failed.
    PostConstruct(String),
    /// A plugin's `build` returned an error; startup is aborted.
    PluginBuild {
        plugin: &'static str,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl fmt::Display for BeanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BeanError::CyclicDependency { cycle } => {
                write!(f, "Circular dependency detected: {}", cycle.join(" -> "))
            }
            BeanError::MissingDependency { bean, dependency } => {
                write!(
                    f,
                    "Missing dependency for bean '{}': type '{}' is not registered. \
                     Use .provide(instance) or .register::<Type>()",
                    bean, dependency
                )
            }
            BeanError::DuplicateBean { type_name } => {
                write!(
                    f,
                    "Bean of type '{}' is registered more than once. Remove the \
                     duplicate .register()/.provide(). For an intentional override, \
                     register the base with .with_default_bean() (last-wins); in \
                     tests, pin a replacement with .override_bean()",
                    type_name
                )
            }
            BeanError::DuplicatePlugin { plugin, owners } => {
                let rendered = match owners.split_last() {
                    Some((last, [])) => format!("by {last}"),
                    Some((last, head)) => format!(
                        "by {} and by {last}",
                        head.iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            .join(", by ")
                    ),
                    None => "more than once".to_string(),
                };
                write!(
                    f,
                    "Plugin '{plugin}' is installed {rendered}. A plugin has exactly one \
                     owner — the app or ONE module. Use `requires_plugins({plugin})` in \
                     every module that only needs it, and keep the single `.plugin({plugin})` \
                     / `plugins({plugin} = ..)` install."
                )
            }
            BeanError::MissingConfigKeys(err) => {
                write!(f, "{}", err)
            }
            BeanError::PostConstruct(msg) => {
                write!(f, "Post-construct hook failed: {}", msg)
            }
            BeanError::PluginBuild { plugin, source } => {
                write!(f, "Plugin '{}' failed to build: {}", plugin, source)
            }
        }
    }
}

impl std::error::Error for BeanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BeanError::PluginBuild { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}
