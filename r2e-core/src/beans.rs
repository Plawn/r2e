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
    pub async fn resolve(self) -> Result<BeanContext, BeanError> {
        let mut entries: HashMap<TypeId, Box<dyn Any + Send + Sync>> = HashMap::new();

        // Move provided instances into the resolved set.
        for (tid, value) in self.provided {
            entries.insert(tid, value);
        }

        let bean_count = self.beans.len();
        if bean_count == 0 {
            return Ok(BeanContext { entries });
        }

        // Check for duplicates
        Self::check_for_duplicates(&self.beans, &entries)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Dep {
        value: i32,
    }

    #[derive(Clone)]
    struct ServiceA {
        dep: Dep,
    }

    impl Bean for ServiceA {
        fn dependencies() -> Vec<(TypeId, &'static str)> {
            vec![(TypeId::of::<Dep>(), type_name::<Dep>())]
        }
        fn build(ctx: &BeanContext) -> Self {
            Self {
                dep: ctx.get::<Dep>(),
            }
        }
    }

    #[derive(Clone)]
    struct ServiceB {
        a: ServiceA,
        dep: Dep,
    }

    impl Bean for ServiceB {
        fn dependencies() -> Vec<(TypeId, &'static str)> {
            vec![
                (TypeId::of::<ServiceA>(), type_name::<ServiceA>()),
                (TypeId::of::<Dep>(), type_name::<Dep>()),
            ]
        }
        fn build(ctx: &BeanContext) -> Self {
            Self {
                a: ctx.get::<ServiceA>(),
                dep: ctx.get::<Dep>(),
            }
        }
    }

    #[tokio::test]
    async fn resolve_simple_graph() {
        let mut reg = BeanRegistry::new();
        reg.provide(Dep { value: 42 });
        reg.register::<ServiceA>();
        reg.register::<ServiceB>();
        let ctx = reg.resolve().await.unwrap();

        let b: ServiceB = ctx.get();
        assert_eq!(b.dep.value, 42);
        assert_eq!(b.a.dep.value, 42);
    }

    #[tokio::test]
    async fn missing_dependency() {
        let mut reg = BeanRegistry::new();
        reg.register::<ServiceA>();
        let err = reg.resolve().await.unwrap_err();
        match &err {
            BeanError::MissingDependency { dependency, .. } => {
                assert!(dependency.contains("Dep"), "error should name the missing type: {}", err);
            }
            _ => panic!("expected MissingDependency, got {:?}", err),
        }
    }

    #[tokio::test]
    async fn duplicate_bean_registered_twice() {
        let mut reg = BeanRegistry::new();
        reg.provide(Dep { value: 1 });
        reg.register::<ServiceA>();
        reg.register::<ServiceA>();
        let err = reg.resolve().await.unwrap_err();
        assert!(matches!(err, BeanError::DuplicateBean { .. }));
    }

    #[tokio::test]
    async fn duplicate_provided_and_bean() {
        let mut reg = BeanRegistry::new();
        reg.provide(Dep { value: 1 });
        reg.provide(ServiceA {
            dep: Dep { value: 2 },
        });
        reg.register::<ServiceA>();
        let err = reg.resolve().await.unwrap_err();
        assert!(matches!(err, BeanError::DuplicateBean { .. }));
    }

    #[derive(Clone)]
    struct CycleA;
    #[derive(Clone)]
    struct CycleB;

    impl Bean for CycleA {
        fn dependencies() -> Vec<(TypeId, &'static str)> {
            vec![(TypeId::of::<CycleB>(), type_name::<CycleB>())]
        }
        fn build(ctx: &BeanContext) -> Self {
            let _ = ctx.get::<CycleB>();
            Self
        }
    }
    impl Bean for CycleB {
        fn dependencies() -> Vec<(TypeId, &'static str)> {
            vec![(TypeId::of::<CycleA>(), type_name::<CycleA>())]
        }
        fn build(ctx: &BeanContext) -> Self {
            let _ = ctx.get::<CycleA>();
            Self
        }
    }

    #[tokio::test]
    async fn cyclic_dependency() {
        let mut reg = BeanRegistry::new();
        reg.register::<CycleA>();
        reg.register::<CycleB>();
        let err = reg.resolve().await.unwrap_err();
        assert!(matches!(err, BeanError::CyclicDependency { .. }));
    }

    #[tokio::test]
    async fn provided_only() {
        let mut reg = BeanRegistry::new();
        reg.provide(Dep { value: 7 });
        let ctx = reg.resolve().await.unwrap();
        let d: Dep = ctx.get();
        assert_eq!(d.value, 7);
    }

    #[tokio::test]
    async fn try_get_none() {
        let reg = BeanRegistry::new();
        let ctx = reg.resolve().await.unwrap();
        assert!(ctx.try_get::<Dep>().is_none());
    }

    #[tokio::test]
    async fn empty_registry() {
        let reg = BeanRegistry::new();
        let ctx = reg.resolve().await.unwrap();
        assert!(ctx.try_get::<i32>().is_none());
    }

    // ── Async bean tests ──────────────────────────────────────────────────

    #[derive(Clone)]
    struct AsyncService {
        dep: Dep,
    }

    impl AsyncBean for AsyncService {
        fn dependencies() -> Vec<(TypeId, &'static str)> {
            vec![(TypeId::of::<Dep>(), type_name::<Dep>())]
        }
        async fn build(ctx: &BeanContext) -> Self {
            // Simulate async init
            tokio::task::yield_now().await;
            Self {
                dep: ctx.get::<Dep>(),
            }
        }
    }

    #[tokio::test]
    async fn async_bean_resolution() {
        let mut reg = BeanRegistry::new();
        reg.provide(Dep { value: 99 });
        reg.register_async::<AsyncService>();
        let ctx = reg.resolve().await.unwrap();

        let svc: AsyncService = ctx.get();
        assert_eq!(svc.dep.value, 99);
    }

    #[tokio::test]
    async fn mixed_sync_async_graph() {
        let mut reg = BeanRegistry::new();
        reg.provide(Dep { value: 10 });
        reg.register::<ServiceA>();          // sync: depends on Dep
        reg.register_async::<AsyncService>(); // async: depends on Dep
        let ctx = reg.resolve().await.unwrap();

        let a: ServiceA = ctx.get();
        let svc: AsyncService = ctx.get();
        assert_eq!(a.dep.value, 10);
        assert_eq!(svc.dep.value, 10);
    }

    // ── Producer tests ────────────────────────────────────────────────────

    #[derive(Clone)]
    struct DbPool {
        url: String,
    }

    struct CreateDbPool;

    impl Producer for CreateDbPool {
        type Output = DbPool;

        fn dependencies() -> Vec<(TypeId, &'static str)> {
            vec![]
        }

        async fn produce(_ctx: &BeanContext) -> DbPool {
            // Simulate async pool creation
            tokio::task::yield_now().await;
            DbPool {
                url: "sqlite::memory:".to_string(),
            }
        }
    }

    #[tokio::test]
    async fn producer_resolution() {
        let mut reg = BeanRegistry::new();
        reg.register_producer::<CreateDbPool>();
        let ctx = reg.resolve().await.unwrap();

        let pool: DbPool = ctx.get();
        assert_eq!(pool.url, "sqlite::memory:");
    }

    #[tokio::test]
    async fn producer_as_dependency() {
        // Producer creates DbPool, then a sync bean depends on it.
        #[derive(Clone)]
        struct RepoService {
            pool: DbPool,
        }

        impl Bean for RepoService {
            fn dependencies() -> Vec<(TypeId, &'static str)> {
                vec![(TypeId::of::<DbPool>(), type_name::<DbPool>())]
            }
            fn build(ctx: &BeanContext) -> Self {
                Self {
                    pool: ctx.get::<DbPool>(),
                }
            }
        }

        let mut reg = BeanRegistry::new();
        reg.register_producer::<CreateDbPool>();
        reg.register::<RepoService>();
        let ctx = reg.resolve().await.unwrap();

        let repo: RepoService = ctx.get();
        assert_eq!(repo.pool.url, "sqlite::memory:");
    }
}
