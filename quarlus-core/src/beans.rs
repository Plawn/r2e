use std::any::{type_name, Any, TypeId};
use std::collections::HashMap;
use std::fmt;

// ── Traits ──────────────────────────────────────────────────────────────────

/// Marker trait for types that can be auto-constructed from a [`BeanContext`].
///
/// Implement this trait (or use `#[derive(Bean)]` / `#[bean]`) to declare
/// a type as a bean that the [`BeanRegistry`] can resolve automatically.
pub trait Bean: Clone + Send + Sync + 'static {
    /// Returns the [`TypeId`]s of all dependencies needed to construct this bean.
    fn dependencies() -> Vec<TypeId>;

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
    instances: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl BeanContext {
    /// Retrieve a bean by type, cloning it out of the context.
    ///
    /// # Panics
    ///
    /// Panics if the requested type was not registered or provided.
    pub fn get<T: Clone + 'static>(&self) -> T {
        self.instances
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
        self.instances
            .get(&TypeId::of::<T>())
            .and_then(|v| v.downcast_ref::<T>())
            .cloned()
    }
}

// ── BeanRegistry ────────────────────────────────────────────────────────────

/// Builder that collects bean registrations and provided instances,
/// resolves the dependency graph, and produces a [`BeanContext`].
pub struct BeanRegistry {
    beans: Vec<BeanRegistration>,
    provided: HashMap<TypeId, ProvidedEntry>,
}

struct BeanRegistration {
    type_id: TypeId,
    type_name: &'static str,
    dependencies: Vec<TypeId>,
    factory: Box<dyn Fn(&BeanContext) -> Box<dyn Any + Send + Sync> + Send + Sync>,
}

struct ProvidedEntry {
    type_name: &'static str,
    value: Box<dyn Any + Send + Sync>,
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
        self.provided.insert(
            TypeId::of::<T>(),
            ProvidedEntry {
                type_name: type_name::<T>(),
                value: Box::new(value),
            },
        );
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
        let mut instances: HashMap<TypeId, Box<dyn Any + Send + Sync>> = HashMap::new();

        // Move provided instances into the resolved set.
        for (tid, entry) in self.provided {
            instances.insert(tid, entry.value);
        }

        // Check for duplicates: a bean type registered twice, or a bean
        // that is also provided.
        let mut seen: HashMap<TypeId, &str> = HashMap::new();
        for reg in &self.beans {
            if instances.contains_key(&reg.type_id) {
                return Err(BeanError::DuplicateBean {
                    type_name: reg.type_name.to_string(),
                });
            }
            if let Some(prev) = seen.insert(reg.type_id, reg.type_name) {
                let _ = prev;
                return Err(BeanError::DuplicateBean {
                    type_name: reg.type_name.to_string(),
                });
            }
        }

        // Build a name lookup for error messages.
        let type_names: HashMap<TypeId, &str> = self
            .beans
            .iter()
            .map(|r| (r.type_id, r.type_name))
            .collect();

        // Check for missing dependencies.
        for reg in &self.beans {
            for dep in &reg.dependencies {
                if !instances.contains_key(dep)
                    && !self.beans.iter().any(|b| b.type_id == *dep)
                {
                    let dep_name = type_names
                        .get(dep)
                        .copied()
                        .unwrap_or("<unknown>");
                    return Err(BeanError::MissingDependency {
                        bean: reg.type_name.to_string(),
                        dependency: dep_name.to_string(),
                    });
                }
            }
        }

        // Kahn's algorithm: topological sort.
        let bean_count = self.beans.len();

        // Map TypeId -> index for beans.
        let id_to_idx: HashMap<TypeId, usize> = self
            .beans
            .iter()
            .enumerate()
            .map(|(i, r)| (r.type_id, i))
            .collect();

        // Compute in-degree (number of unresolved deps per bean).
        let mut in_degree: Vec<usize> = Vec::with_capacity(bean_count);
        for reg in &self.beans {
            let deg = reg
                .dependencies
                .iter()
                .filter(|d| id_to_idx.contains_key(d))
                .count();
            in_degree.push(deg);
        }

        // Dependents: for each bean, which other beans depend on it.
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); bean_count];
        for (i, reg) in self.beans.iter().enumerate() {
            for dep in &reg.dependencies {
                if let Some(&dep_idx) = id_to_idx.get(dep) {
                    dependents[dep_idx].push(i);
                }
            }
        }

        // Queue starts with beans that have all deps already resolved (in provided).
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
                .map(|i| self.beans[i].type_name.to_string())
                .collect();
            return Err(BeanError::CyclicDependency { cycle });
        }

        // Construct beans in topological order.
        // We need to move factories out of self.beans, so collect them.
        let mut factories: Vec<Option<Box<dyn Fn(&BeanContext) -> Box<dyn Any + Send + Sync> + Send + Sync>>> =
            self.beans.into_iter().map(|r| Some(r.factory)).collect();

        for idx in sorted_order {
            let factory = factories[idx].take().unwrap();
            let ctx = BeanContext {
                instances: instances
                    .iter()
                    .map(|(k, v)| {
                        // We need to clone the Any values, but we can't clone Box<dyn Any>.
                        // Instead, we create a temporary context with references.
                        // Actually, we need a different approach — pass a read-only view.
                        (*k, v)
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .collect::<HashMap<_, _>>()
                    .into_iter()
                    .map(|_| unreachable!())
                    .collect(),
            };
            // The above doesn't work — we need to restructure.
            // Let's just build context from current instances directly.
            let _ = ctx;
            let bean_instance = {
                // Build a temporary BeanContext that borrows from instances.
                // Since BeanContext owns its data, we need to take a snapshot.
                // The Bean::build() will call ctx.get::<T>() which clones.
                // We can't easily share Box<dyn Any>, so we use a different approach:
                // store all instances as type-erased cloneable values.
                //
                // Actually, the simplest approach: since every bean is Clone,
                // we can store cloning closures. But that complicates the API.
                //
                // Simpler: just pass instances by reference using an internal type.
                factory(&BeanContext { instances: HashMap::new() })
            };
            let _ = bean_instance;
            todo!()
        }

        Ok(BeanContext { instances })
    }
}

impl Default for BeanRegistry {
    fn default() -> Self {
        Self::new()
    }
}
