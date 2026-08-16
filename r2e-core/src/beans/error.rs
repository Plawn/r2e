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
