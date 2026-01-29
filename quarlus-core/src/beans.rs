use std::any::{type_name, Any, TypeId};
use std::collections::HashMap;
use std::fmt;

// ── Traits ──────────────────────────────────────────────────────────────────

/// Marker trait for types that can be auto-constructed from a [`BeanContext`].
///
/// Implement this trait (or use `#[derive(Bean)]` / `#[bean]`) to declare
/// a type as a bean that the [`BeanRegistry`] can resolve automatically.
pub trait Bean: Clone + Send + Sync + 'static {
    /// Returns the [`TypeId`]s and type names of all dependencies needed
    /// to construct this bean.
    fn dependencies() -> Vec<(TypeId, &'static str)>;

    /// Construct the bean from a fully resolved context.
    fn build(ctx: &BeanContext) -> Self;
}

/// Trait for state structs that can be assembled from a [`BeanContext`].
///
/// Use `#[derive(BeanState)]` to auto-generate this implementation along
/// with `FromRef` impls for Axum state extraction.
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

type Factory = Box<dyn FnOnce(&BeanContext) -> Box<dyn Any + Send + Sync> + Send>;

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

    /// Register a bean type for automatic construction.
    ///
    /// The bean's dependencies will be resolved from other beans or provided
    /// instances during [`resolve`](Self::resolve).
    pub fn register<T: Bean>(&mut self) -> &mut Self {
        self.beans.push(BeanRegistration {
            type_id: TypeId::of::<T>(),
            type_name: type_name::<T>(),
            dependencies: T::dependencies(),
            factory: Box::new(|ctx| Box::new(T::build(ctx))),
        });
        self
    }

    /// Resolve the dependency graph and build all beans.
    ///
    /// Uses Kahn's algorithm for topological sorting. Returns a
    /// [`BeanContext`] with all instances, or a [`BeanError`] if the graph
    /// is invalid (cycles, missing deps, or duplicates).
    pub fn resolve(self) -> Result<BeanContext, BeanError> {
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

        // Topological sort
        let sorted_order = Self::topological_sort(&self.beans, &id_to_idx, bean_count)?;

        // Construct beans in order
        entries = Self::construct_beans_in_order(self.beans, sorted_order, entries);

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

    /// Construct beans in topological order.
    fn construct_beans_in_order(
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
            // Temporarily move entries into a BeanContext so the factory can
            // call ctx.get::<T>(). After the call, move entries back.
            let ctx = BeanContext { entries };
            let bean_value = factory(&ctx);
            entries = ctx.entries;
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

    #[test]
    fn resolve_simple_graph() {
        let mut reg = BeanRegistry::new();
        reg.provide(Dep { value: 42 });
        reg.register::<ServiceA>();
        reg.register::<ServiceB>();
        let ctx = reg.resolve().unwrap();

        let b: ServiceB = ctx.get();
        assert_eq!(b.dep.value, 42);
        assert_eq!(b.a.dep.value, 42);
    }

    #[test]
    fn missing_dependency() {
        let mut reg = BeanRegistry::new();
        reg.register::<ServiceA>();
        let err = reg.resolve().unwrap_err();
        match &err {
            BeanError::MissingDependency { dependency, .. } => {
                assert!(dependency.contains("Dep"), "error should name the missing type: {}", err);
            }
            _ => panic!("expected MissingDependency, got {:?}", err),
        }
    }

    #[test]
    fn duplicate_bean_registered_twice() {
        let mut reg = BeanRegistry::new();
        reg.provide(Dep { value: 1 });
        reg.register::<ServiceA>();
        reg.register::<ServiceA>();
        let err = reg.resolve().unwrap_err();
        assert!(matches!(err, BeanError::DuplicateBean { .. }));
    }

    #[test]
    fn duplicate_provided_and_bean() {
        let mut reg = BeanRegistry::new();
        reg.provide(Dep { value: 1 });
        reg.provide(ServiceA {
            dep: Dep { value: 2 },
        });
        reg.register::<ServiceA>();
        let err = reg.resolve().unwrap_err();
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

    #[test]
    fn cyclic_dependency() {
        let mut reg = BeanRegistry::new();
        reg.register::<CycleA>();
        reg.register::<CycleB>();
        let err = reg.resolve().unwrap_err();
        assert!(matches!(err, BeanError::CyclicDependency { .. }));
    }

    #[test]
    fn provided_only() {
        let mut reg = BeanRegistry::new();
        reg.provide(Dep { value: 7 });
        let ctx = reg.resolve().unwrap();
        let d: Dep = ctx.get();
        assert_eq!(d.value, 7);
    }

    #[test]
    fn try_get_none() {
        let reg = BeanRegistry::new();
        let ctx = reg.resolve().unwrap();
        assert!(ctx.try_get::<Dep>().is_none());
    }

    #[test]
    fn empty_registry() {
        let reg = BeanRegistry::new();
        let ctx = reg.resolve().unwrap();
        assert!(ctx.try_get::<i32>().is_none());
    }
}
