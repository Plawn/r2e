use std::any::{type_name, Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;

// ── Traits ──────────────────────────────────────────────────────────────────

/// Marker trait for types that can be auto-constructed from a [`BeanContext`].
///
/// Implement this trait (or use `#[derive(Bean)]` / `#[bean]`) to declare
/// a type as a bean that the [`BeanRegistry`] can resolve automatically.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not registered as a Bean",
    label = "this type is not a bean",
    note = "add `#[derive(Bean)]` to your type or implement the `Bean` trait manually"
)]
pub trait Bean: Clone + Send + Sync + 'static {
    /// Type-level list of dependency types required to construct this bean.
    ///
    /// Generated automatically by `#[bean]` and `#[derive(Bean)]`.
    /// For manual impls without dependencies, use `type Deps = TNil;`.
    type Deps;

    /// Returns the [`TypeId`]s and type names of all dependencies needed
    /// to construct this bean.
    fn dependencies() -> Vec<(TypeId, &'static str)>;

    /// Returns the config keys required by this bean as `(key, type_name)` pairs.
    ///
    /// Used by [`BeanRegistry::resolve`] to validate all config keys before
    /// constructing any bean. The default implementation returns an empty list.
    fn config_keys() -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }

    /// Construct the bean from a fully resolved context.
    fn build(ctx: &BeanContext) -> Self;
}

/// Trait for beans that require async initialization (e.g. DB pools, HTTP clients).
///
/// Use `#[bean]` on an `impl` block with an `async fn new(...)` constructor,
/// or implement this trait manually. Register with `.with_async_bean::<T>()`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not registered as an AsyncBean",
    label = "this type is not an async bean",
    note = "add `#[bean]` to your impl block with an `async fn` constructor, or implement `AsyncBean` manually"
)]
pub trait AsyncBean: Clone + Send + Sync + 'static {
    /// Type-level list of dependency types required to construct this bean.
    ///
    /// Generated automatically by `#[bean]` on async constructors.
    /// For manual impls without dependencies, use `type Deps = TNil;`.
    type Deps;

    /// Returns the [`TypeId`]s and type names of all dependencies needed
    /// to construct this bean.
    fn dependencies() -> Vec<(TypeId, &'static str)>;

    /// Returns the config keys required by this bean as `(key, type_name)` pairs.
    ///
    /// Used by [`BeanRegistry::resolve`] to validate all config keys before
    /// constructing any bean. The default implementation returns an empty list.
    fn config_keys() -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }

    /// Construct the bean asynchronously from a fully resolved context.
    fn build(ctx: &BeanContext) -> impl Future<Output = Self> + Send + '_;
}

/// Trait for producer functions that create types you don't own
/// (e.g. `SqlitePool`, third-party clients).
///
/// Use the `#[producer]` attribute macro on a free function to generate
/// this implementation automatically. Register with `.with_producer::<P>()`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not registered as a Producer",
    label = "this type is not a producer",
    note = "add `#[producer]` to a free function that returns the desired type"
)]
pub trait Producer: Send + 'static {
    /// The type this producer creates.
    type Output: Clone + Send + Sync + 'static;

    /// Type-level list of dependency types required to produce the output.
    ///
    /// Generated automatically by `#[producer]`.
    /// For manual impls without dependencies, use `type Deps = TNil;`.
    type Deps;

    /// Returns the [`TypeId`]s and type names of all dependencies needed
    /// to produce the output.
    fn dependencies() -> Vec<(TypeId, &'static str)>;

    /// Returns the config keys required by this producer as `(key, type_name)` pairs.
    ///
    /// Used by [`BeanRegistry::resolve`] to validate all config keys before
    /// constructing any bean. The default implementation returns an empty list.
    fn config_keys() -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }

    /// Produce the output from a fully resolved context.
    fn produce(ctx: &BeanContext) -> impl Future<Output = Self::Output> + Send + '_;
}

/// Trait for state structs that can be assembled from a [`BeanContext`].
///
/// Use `#[derive(BeanState)]` to auto-generate this implementation along
/// with `FromRef` impls for Axum state extraction.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `BeanState`",
    label = "this type cannot be used as bean state",
    note = "add `#[derive(BeanState)]` to your state struct"
)]
pub trait BeanState: Clone + Send + Sync + 'static {
    /// Construct the state struct by pulling every field from the context.
    fn from_context(ctx: &BeanContext) -> Self;
}

// ── BeanContext ─────────────────────────────────────────────────────────────

/// Read-only container holding all resolved bean instances.
///
/// Produced by [`BeanRegistry::resolve`]. Each entry is keyed by [`TypeId`].
pub struct BeanContext {
    entries: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl fmt::Debug for BeanContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BeanContext")
            .field("entry_count", &self.entries.len())
            .finish()
    }
}

impl BeanContext {
    /// Retrieve a bean by type, cloning it out of the context.
    ///
    /// # Panics
    ///
    /// Panics if the requested type was not registered or provided.
    pub fn get<T: Clone + 'static>(&self) -> T {
        self.entries
            .get(&TypeId::of::<T>())
            .and_then(|v| v.downcast_ref::<T>())
            .unwrap_or_else(|| {
                panic!(
                    "Bean of type `{}` not found in context",
                    type_name::<T>()
                )
            })
            .clone()
    }

    /// Try to retrieve a bean by type, returning `None` if absent.
    pub fn try_get<T: Clone + 'static>(&self) -> Option<T> {
        self.entries
            .get(&TypeId::of::<T>())
            .and_then(|v| v.downcast_ref::<T>())
            .cloned()
    }
}

// ── BeanRegistry ────────────────────────────────────────────────────────────

/// Async factory: takes BeanContext by value (to avoid lifetime issues with
/// async captures), returns the context back along with the constructed bean.
type Factory = Box<
    dyn FnOnce(
            BeanContext,
        ) -> Pin<
            Box<dyn Future<Output = (BeanContext, Box<dyn Any + Send + Sync>)> + Send>,
        > + Send,
>;

/// Builder that collects bean registrations and provided instances,
/// resolves the dependency graph, and produces a [`BeanContext`].
pub struct BeanRegistry {
    beans: Vec<BeanRegistration>,
    provided: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    /// When true, duplicate bean registrations are allowed (last wins).
    pub(crate) allow_overrides: bool,
}

struct BeanRegistration {
    type_id: TypeId,
    type_name: &'static str,
    /// (TypeId, human-readable name) for each dependency.
    dependencies: Vec<(TypeId, &'static str)>,
    /// (config_key, expected_type_name) for config validation.
    config_keys: Vec<(&'static str, &'static str)>,
    factory: Factory,
}

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
}

impl fmt::Display for BeanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BeanError::CyclicDependency { cycle } => {
                write!(
                    f,
                    "Circular dependency detected: {}",
                    cycle.join(" -> ")
                )
            }
            BeanError::MissingDependency { bean, dependency } => {
                write!(
                    f,
                    "Missing dependency for bean '{}': type '{}' is not registered. \
                     Use .provide(instance) or .with_bean::<Type>()",
                    bean, dependency
                )
            }
            BeanError::DuplicateBean { type_name } => {
                write!(f, "Bean of type '{}' registered twice", type_name)
            }
            BeanError::MissingConfigKeys(err) => {
                write!(f, "{}", err)
            }
        }
    }
}

impl std::error::Error for BeanError {}

impl BeanRegistry {
    /// Create a new, empty registry.
    pub fn new() -> Self {
        Self {
            beans: Vec::new(),
            provided: HashMap::new(),
            allow_overrides: false,
        }
    }

    /// Provide a pre-built instance (e.g. external types like `SqlitePool`).
    ///
    /// The instance will be available to beans that depend on type `T`.
    pub fn provide<T: Clone + Send + Sync + 'static>(&mut self, value: T) -> &mut Self {
        self.provided.insert(TypeId::of::<T>(), Box::new(value));
        self
    }

    /// Register a (sync) bean type for automatic construction.
    ///
    /// The bean's dependencies will be resolved from other beans or provided
    /// instances during [`resolve`](Self::resolve).
    pub fn register<T: Bean>(&mut self) -> &mut Self {
        self.beans.push(BeanRegistration {
            type_id: TypeId::of::<T>(),
            type_name: type_name::<T>(),
            dependencies: T::dependencies(),
            config_keys: T::config_keys(),
            factory: Box::new(|ctx| {
                Box::pin(async move {
                    let bean = T::build(&ctx);
                    (ctx, Box::new(bean) as Box<dyn Any + Send + Sync>)
                })
            }),
        });
        self
    }

    /// Register an async bean type for automatic construction.
    ///
    /// The bean's constructor is awaited during resolution.
    pub fn register_async<T: AsyncBean>(&mut self) -> &mut Self {
        self.beans.push(BeanRegistration {
            type_id: TypeId::of::<T>(),
            type_name: type_name::<T>(),
            dependencies: T::dependencies(),
            config_keys: T::config_keys(),
            factory: Box::new(|ctx| {
                Box::pin(async move {
                    let bean = T::build(&ctx).await;
                    (ctx, Box::new(bean) as Box<dyn Any + Send + Sync>)
                })
            }),
        });
        self
    }

    /// Register a bean via factory closure that receives `R2eConfig`.
    ///
    /// The closure is invoked during [`resolve`](Self::resolve) after all
    /// dependencies (including `R2eConfig`) are available.
    ///
    /// This is the underlying method for [`AppBuilder::with_bean_factory`].
    pub fn provide_factory_with_config<T, F>(&mut self, factory: F)
    where
        T: Clone + Send + Sync + 'static,
        F: FnOnce(&crate::config::R2eConfig) -> T + Send + 'static,
    {
        self.beans.push(BeanRegistration {
            type_id: TypeId::of::<T>(),
            type_name: type_name::<T>(),
            dependencies: vec![(
                TypeId::of::<crate::config::R2eConfig>(),
                "R2eConfig",
            )],
            config_keys: vec![],
            factory: Box::new(move |ctx| {
                Box::pin(async move {
                    let config = ctx.get::<crate::config::R2eConfig>();
                    let bean = factory(&config);
                    (ctx, Box::new(bean) as Box<dyn Any + Send + Sync>)
                })
            }),
        });
    }

    /// Register a producer for automatic construction of its output type.
    ///
    /// The producer is awaited during resolution. The resulting bean is
    /// registered under the producer's `Output` type.
    pub fn register_producer<P: Producer>(&mut self) -> &mut Self {
        self.beans.push(BeanRegistration {
            type_id: TypeId::of::<P::Output>(),
            type_name: type_name::<P::Output>(),
            dependencies: P::dependencies(),
            config_keys: P::config_keys(),
            factory: Box::new(|ctx| {
                Box::pin(async move {
                    let output = P::produce(&ctx).await;
                    (ctx, Box::new(output) as Box<dyn Any + Send + Sync>)
                })
            }),
        });
        self
    }

    /// Resolve the dependency graph and build all beans.
    ///
    /// Uses Kahn's algorithm for topological sorting. Returns a
    /// [`BeanContext`] with all instances, or a [`BeanError`] if the graph
    /// is invalid (cycles, missing deps, or duplicates).
    pub async fn resolve(mut self) -> Result<BeanContext, BeanError> {
        let mut entries: HashMap<TypeId, Box<dyn Any + Send + Sync>> = HashMap::new();

        // Move provided instances into the resolved set.
        for (tid, value) in self.provided {
            entries.insert(tid, value);
        }

        // When overrides are allowed, deduplicate beans (last registration wins).
        if self.allow_overrides {
            Self::deduplicate_beans(&mut self.beans, &mut entries);
        }

        let bean_count = self.beans.len();
        if bean_count == 0 {
            return Ok(BeanContext { entries });
        }

        // Check for duplicates
        if !self.allow_overrides {
            Self::check_for_duplicates(&self.beans, &entries)?;
        }

        // Build dependency graph
        let id_to_idx = Self::build_type_index(&self.beans);
        Self::check_missing_dependencies(&self.beans, &entries, &id_to_idx)?;

        // Validate config keys before construction
        Self::validate_config_keys(&self.beans, &entries)?;

        // Topological sort
        let sorted_order = Self::topological_sort(&self.beans, &id_to_idx, bean_count)?;

        // Construct beans in order (async)
        entries = Self::construct_beans_in_order(self.beans, sorted_order, entries).await;

        Ok(BeanContext { entries })
    }

    /// Deduplicate beans when overrides are enabled: last registration wins.
    /// Also removes beans whose type_id already exists in provided entries.
    fn deduplicate_beans(
        beans: &mut Vec<BeanRegistration>,
        entries: &mut HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    ) {
        // Remove beans that conflict with provided instances (provided wins by default,
        // but with overrides a later bean should win over an earlier provide).
        // Strategy: iterate in order; for each type_id, keep the last occurrence.
        let mut last_seen: HashMap<TypeId, usize> = HashMap::new();
        for (i, reg) in beans.iter().enumerate() {
            last_seen.insert(reg.type_id, i);
        }
        let keep: std::collections::HashSet<usize> = last_seen.values().copied().collect();
        let mut idx = 0;
        beans.retain(|_| {
            let kept = keep.contains(&idx);
            idx += 1;
            kept
        });
        // If a bean type also exists in provided, remove the provided entry
        // (the bean factory will be constructed instead).
        for reg in beans.iter() {
            entries.remove(&reg.type_id);
        }
    }

    /// Check for duplicate bean registrations.
    fn check_for_duplicates(
        beans: &[BeanRegistration],
        entries: &HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    ) -> Result<(), BeanError> {
        let mut seen: HashMap<TypeId, &str> = HashMap::new();
        for reg in beans {
            if entries.contains_key(&reg.type_id) {
                return Err(BeanError::DuplicateBean {
                    type_name: reg.type_name.to_string(),
                });
            }
            if seen.insert(reg.type_id, reg.type_name).is_some() {
                return Err(BeanError::DuplicateBean {
                    type_name: reg.type_name.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Build a map from TypeId to bean index.
    fn build_type_index(beans: &[BeanRegistration]) -> HashMap<TypeId, usize> {
        beans
            .iter()
            .enumerate()
            .map(|(i, r)| (r.type_id, i))
            .collect()
    }

    /// Check that all dependencies are available.
    fn check_missing_dependencies(
        beans: &[BeanRegistration],
        entries: &HashMap<TypeId, Box<dyn Any + Send + Sync>>,
        id_to_idx: &HashMap<TypeId, usize>,
    ) -> Result<(), BeanError> {
        for reg in beans {
            for (dep_id, dep_name) in &reg.dependencies {
                if !entries.contains_key(dep_id) && !id_to_idx.contains_key(dep_id) {
                    return Err(BeanError::MissingDependency {
                        bean: reg.type_name.to_string(),
                        dependency: dep_name.to_string(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Validate all config keys declared by beans against the provided R2eConfig.
    ///
    /// Uses the shared [`validate_keys`](crate::config::validate_keys) function.
    fn validate_config_keys(
        beans: &[BeanRegistration],
        entries: &HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    ) -> Result<(), BeanError> {
        let all_keys: Vec<_> = beans
            .iter()
            .flat_map(|reg| {
                reg.config_keys
                    .iter()
                    .map(move |(key, ty_name)| (reg.type_name, *key, *ty_name))
            })
            .collect();

        if all_keys.is_empty() {
            return Ok(());
        }

        let config = entries
            .get(&TypeId::of::<crate::config::R2eConfig>())
            .and_then(|v| v.downcast_ref::<crate::config::R2eConfig>());

        let Some(config) = config else {
            return Ok(());
        };

        let errors = crate::config::validate_keys(config, &all_keys);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(BeanError::MissingConfigKeys(
                crate::config::ConfigValidationError { errors },
            ))
        }
    }

    /// Perform topological sort using Kahn's algorithm.
    fn topological_sort(
        beans: &[BeanRegistration],
        id_to_idx: &HashMap<TypeId, usize>,
        bean_count: usize,
    ) -> Result<Vec<usize>, BeanError> {
        // in_degree = number of deps that are other beans (not provided).
        let mut in_degree: Vec<usize> = beans
            .iter()
            .map(|reg| {
                reg.dependencies
                    .iter()
                    .filter(|(d, _)| id_to_idx.contains_key(d))
                    .count()
            })
            .collect();

        // Dependents: for each bean index, which other bean indices depend on it.
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); bean_count];
        for (i, reg) in beans.iter().enumerate() {
            for (dep_id, _) in &reg.dependencies {
                if let Some(&dep_idx) = id_to_idx.get(dep_id) {
                    dependents[dep_idx].push(i);
                }
            }
        }

        // Seed queue with beans whose deps are all already provided.
        let mut queue: Vec<usize> = (0..bean_count)
            .filter(|&i| in_degree[i] == 0)
            .collect();

        let mut sorted_order: Vec<usize> = Vec::with_capacity(bean_count);

        while let Some(idx) = queue.pop() {
            sorted_order.push(idx);
            for &dep_idx in &dependents[idx] {
                in_degree[dep_idx] -= 1;
                if in_degree[dep_idx] == 0 {
                    queue.push(dep_idx);
                }
            }
        }

        // If not all beans were sorted, there's a cycle.
        if sorted_order.len() != bean_count {
            let cycle: Vec<String> = (0..bean_count)
                .filter(|i| in_degree[*i] > 0)
                .map(|i| beans[i].type_name.to_string())
                .collect();
            return Err(BeanError::CyclicDependency { cycle });
        }

        Ok(sorted_order)
    }

    /// Construct beans in topological order (async).
    async fn construct_beans_in_order(
        beans: Vec<BeanRegistration>,
        sorted_order: Vec<usize>,
        mut entries: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    ) -> HashMap<TypeId, Box<dyn Any + Send + Sync>> {
        // Move factories and type_ids out so we can consume them one by one.
        let mut bean_data: Vec<Option<(TypeId, Factory)>> = beans
            .into_iter()
            .map(|r| Some((r.type_id, r.factory)))
            .collect();

        for idx in sorted_order {
            let (type_id, factory) = bean_data[idx].take().unwrap();
            // Move entries into a BeanContext so the factory can call ctx.get::<T>().
            // The factory returns entries back along with the constructed bean.
            let ctx = BeanContext { entries };
            let (returned_ctx, bean_value) = factory(ctx).await;
            entries = returned_ctx.entries;
            entries.insert(type_id, bean_value);
        }

        entries
    }
}

impl Default for BeanRegistry {
    fn default() -> Self {
        Self::new()
    }
}
