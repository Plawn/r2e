use std::fmt;

/// The error channel of R2E's boot path.
///
/// Every fallible assembly step funnels into this type: config loading
/// ([`load_config`](crate::AppBuilder::load_config), whose failure is recorded
/// and surfaced by [`try_build_state`](crate::AppBuilder::try_build_state)),
/// bean construction ([`Bean::build`](crate::beans::Bean::build),
/// [`AsyncBean::build`](crate::beans::AsyncBean::build),
/// [`Producer::produce`](crate::beans::Producer::produce)), plugin build,
/// module/plugin controller registration, the controller `#[post_construct]` /
/// `#[on_start]` hooks run by
/// [`try_build_with_consumers`](crate::AppBuilder::try_build_with_consumers),
/// and the app itself ([`App::setup`](crate::App::setup) /
/// [`App::build`](crate::App::build), [`launch`](crate::launch)).
///
/// The panicking entry points — `build_state()`, `build_with_consumers()`,
/// `TestApp::boot*` — are thin wrappers over the `try_*` forms that render this
/// error; they exist so the common path needs no `?`, not because those steps
/// bypass the channel.
///
/// It is a plain boxed `std` error on purpose: any `E: std::error::Error +
/// Send + Sync + 'static` converts into it with `?`, and it converts on into
/// `Box<dyn Error>` for a `main` that returns one. Context is added by the
/// framework, not by the box: a failing bean is wrapped in
/// [`BeanError::BeanBuild`], which names the bean and keeps the original error
/// as its [`source`](std::error::Error::source).
///
/// ```ignore
/// #[producer]
/// async fn create_pool(#[config("app.db.url")] url: String) -> Result<PgPool, sqlx::Error> {
///     PgPool::connect(&url).await   // `?`-able, no `process::exit`
/// }
/// ```
pub type BootError = Box<dyn std::error::Error + Send + Sync + 'static>;

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
    /// Configuration could not be loaded or bound: a missing/malformed
    /// requested config file, a failing [`ConfigProvider`], an unresolved
    /// `${...}` placeholder, or a typed section that does not bind.
    ///
    /// Recorded by `load_config()` (which cannot return a `Result` — it is a
    /// type-state transition in the middle of the builder chain) and surfaced
    /// by [`try_build_state`](crate::AppBuilder::try_build_state) before any
    /// bean is constructed.
    ///
    /// [`ConfigProvider`]: crate::config::ConfigProvider
    ConfigLoad {
        /// What was being done ("Failed to load config", …).
        context: &'static str,
        source: BootError,
    },
    /// A controller registered by a feature module or a plugin declares config
    /// keys/sections that fail validation. Carries the same aggregated report
    /// [`MissingConfigKeys`](Self::MissingConfigKeys) does, plus the
    /// controller that declared them.
    ControllerConfig {
        /// The controller type whose declared config failed validation.
        controller: &'static str,
        source: crate::config::ConfigValidationError,
    },
    /// A non-HTTP transport endpoint (a gRPC service today) registered by a
    /// feature module declares config keys/sections that fail validation. The
    /// endpoint peer of [`ControllerConfig`](Self::ControllerConfig).
    EndpointConfig {
        /// The endpoint type whose declared config failed validation.
        endpoint: &'static str,
        source: crate::config::ConfigValidationError,
    },
    /// A feature module declares non-HTTP endpoints whose transport plugin is
    /// not installed — e.g. `grpc_services(..)` without `.plugin(GrpcServer::..)`.
    ///
    /// The generated `#[module]` declaration makes this a compile error (the
    /// transport plugin joins `RequiredPlugins`); this variant is the boot-time
    /// backstop for hand-written `FeatureModule` impls.
    MissingTransportPlugin {
        /// The endpoint type that could not be registered.
        endpoint: &'static str,
        /// The plugin the transport needs, e.g. `"GrpcServer"`.
        plugin: &'static str,
    },
    /// A bean's constructor (or a producer function) returned an error.
    ///
    /// The first such error aborts `build_state()`; every bean already built
    /// in this cycle is dropped as the resolution stack unwinds.
    BeanBuild {
        /// The bean type whose construction failed (a producer is named by
        /// its `Output` type — the type the graph knows it by).
        bean: &'static str,
        source: BootError,
    },
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
            BeanError::ConfigLoad { context, source } => {
                write!(f, "{context}: {source}")
            }
            BeanError::ControllerConfig { controller, source } => {
                write!(
                    f,
                    "\n=== CONFIGURATION ERRORS (controller: {controller}) ===\n\n{source}\n============================\n"
                )
            }
            BeanError::EndpointConfig { endpoint, source } => {
                write!(
                    f,
                    "\n=== CONFIGURATION ERRORS (endpoint: {endpoint}) ===\n\n{source}\n============================\n"
                )
            }
            BeanError::MissingTransportPlugin { endpoint, plugin } => {
                write!(
                    f,
                    "Endpoint '{endpoint}' cannot be registered: the `{plugin}` plugin is not \
                     installed. Install it with `.plugin({plugin}::...)` before the module, or \
                     let the module bring it with `plugins({plugin} = ...)`"
                )
            }
            BeanError::BeanBuild { bean, source } => {
                write!(f, "Bean '{}' failed to build: {}", bean, source)
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
            BeanError::BeanBuild { source, .. } => Some(source.as_ref()),
            BeanError::PluginBuild { source, .. } => Some(source.as_ref()),
            BeanError::ConfigLoad { source, .. } => Some(source.as_ref()),
            BeanError::ControllerConfig { source, .. } => Some(source),
            BeanError::EndpointConfig { source, .. } => Some(source),
            _ => None,
        }
    }
}
