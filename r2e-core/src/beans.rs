use std::any::{type_name, Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
#[cfg(feature = "dev-reload")]
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::{Arc, RwLock};

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
    ///
    /// `Option<T>` fields are **hard** dependencies on `Option<T>` (the
    /// whole type, not `T`). A producer must register an `Option<T>` value
    /// in the context for this bean to resolve. See the module docs for
    /// the conditional-bean pattern using `#[producer] -> Option<T>`.
    fn dependencies() -> Vec<(TypeId, &'static str)>;

    /// Returns the config keys required by this bean as `(key, type_name)` pairs.
    ///
    /// Used by [`BeanRegistry::resolve`] to validate all config keys before
    /// constructing any bean. The default implementation returns an empty list.
    fn config_keys() -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }

    /// When `true`, construction is deferred until first injection.
    /// Set by `#[bean(lazy)]`.
    ///
    /// Lazy beans are **not** constructed during `build_state()`. Instead,
    /// a lazy slot is placed in the context and the bean is built on the
    /// first `get::<Self>()` call (construct-on-first-injection, like
    /// Quarkus CDI).
    ///
    /// **Runtime note:** lazy resolution needs a Tokio multi-thread runtime.
    /// Enable the `lazy-fallback-runtime` feature to allow a fallback runtime
    /// when none is available (or when running on a current-thread runtime).
    ///
    /// Consumers use `Self` directly — no wrapper type needed.
    /// Register with `.register::<T>()`
    /// as usual; the builder auto-detects the `LAZY` flag.
    const LAZY: bool = false;

    /// A version stamp derived from the constructor's source tokens.
    ///
    /// The `#[bean]` and `#[derive(Bean)]` macros hash the constructor body /
    /// struct fields at compile time, so a code change automatically bumps this
    /// value. Used by the dev-reload granular bean cache to detect code changes.
    ///
    /// **Manual implementations:** The default value is `0`, which means the
    /// dev-reload system will **not** detect code changes in your constructor.
    /// If you implement `Bean` manually and want hot-reload to pick up changes,
    /// override this constant and bump it whenever you modify the `build` logic:
    ///
    /// ```ignore
    /// impl Bean for MyService {
    ///     const BUILD_VERSION: u64 = 2; // bump when build() changes
    ///     // ...
    /// }
    /// ```
    const BUILD_VERSION: u64 = 0;

    /// Construct the bean from a fully resolved context.
    fn build(ctx: &BeanContext) -> Self;

    /// Called after registration to allow post-processing (e.g., registering
    /// post-construct hooks). The default is a no-op.
    fn after_register(_registry: &mut BeanRegistry) {}
}

/// Trait for beans that require async initialization (e.g. DB pools, HTTP clients).
///
/// Use `#[bean]` on an `impl` block with an `async fn new(...)` constructor,
/// or implement this trait manually. Register with `.register::<T>()`.
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

    /// When `true`, construction is deferred until first injection.
    /// Set by `#[bean(lazy)]`. See [`Bean::LAZY`] for details.
    const LAZY: bool = false;

    /// Returns the [`TypeId`]s and type names of all dependencies needed
    /// to construct this bean.
    ///
    /// `Option<T>` fields are **hard** dependencies on `Option<T>` (the
    /// whole type, not `T`). A producer must register an `Option<T>` value
    /// in the context for this bean to resolve.
    fn dependencies() -> Vec<(TypeId, &'static str)>;

    /// Returns the config keys required by this bean as `(key, type_name)` pairs.
    ///
    /// Used by [`BeanRegistry::resolve`] to validate all config keys before
    /// constructing any bean. The default implementation returns an empty list.
    fn config_keys() -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }

    /// A version stamp derived from the constructor's source tokens.
    ///
    /// The `#[bean]` macro hashes the async constructor body at compile time,
    /// so a code change automatically bumps this value. Used by the dev-reload
    /// granular bean cache to detect code changes.
    ///
    /// **Manual implementations:** The default value is `0`, which means the
    /// dev-reload system will **not** detect code changes in your constructor.
    /// Override this constant and bump it when you modify `build` logic:
    ///
    /// ```ignore
    /// impl AsyncBean for MyPool {
    ///     const BUILD_VERSION: u64 = 3; // bump when build() changes
    ///     // ...
    /// }
    /// ```
    const BUILD_VERSION: u64 = 0;

    /// Construct the bean asynchronously from a fully resolved context.
    fn build(ctx: &BeanContext) -> impl Future<Output = Self> + Send + '_;

    /// Called after registration to allow post-processing (e.g., registering
    /// post-construct hooks). The default is a no-op.
    fn after_register(_registry: &mut BeanRegistry) {}
}

/// Trait for producer functions that create types you don't own
/// (e.g. `SqlitePool`, third-party clients).
///
/// Use the `#[producer]` attribute macro on a free function to generate
/// this implementation automatically. Register with `.register::<P>()`.
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
    ///
    /// `Option<T>` parameters are **hard** dependencies on `Option<T>`.
    fn dependencies() -> Vec<(TypeId, &'static str)>;

    /// Returns the config keys required by this producer as `(key, type_name)` pairs.
    ///
    /// Used by [`BeanRegistry::resolve`] to validate all config keys before
    /// constructing any bean. The default implementation returns an empty list.
    fn config_keys() -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }

    /// A version stamp derived from the producer function's source tokens.
    ///
    /// The `#[producer]` macro hashes the function body at compile time,
    /// so a code change automatically bumps this value. Used by the dev-reload
    /// granular bean cache to detect code changes.
    ///
    /// **Manual implementations:** The default value is `0`, which means the
    /// dev-reload system will **not** detect code changes in your producer.
    /// Override this constant and bump it when you modify `produce` logic:
    ///
    /// ```ignore
    /// impl Producer for MyProducer {
    ///     const BUILD_VERSION: u64 = 1; // bump when produce() changes
    ///     // ...
    /// }
    /// ```
    const BUILD_VERSION: u64 = 0;

    /// Produce the output from a fully resolved context.
    ///
    /// To express conditional availability (a bean that may or may not be
    /// present depending on config), declare `type Output = Option<T>` and
    /// return `Some(...)` / `None`. The whole `Option<T>` is registered as
    /// a bean — consumers inject `Option<T>` as a hard dependency.
    fn produce(ctx: &BeanContext) -> impl Future<Output = Self::Output> + Send + '_;
}

/// Lifecycle hook called after all beans have been constructed.
///
/// Implement this trait (typically via `#[post_construct]` on a `#[bean]`
/// method) to run initialization logic that requires the fully assembled bean.
/// Per-bean fingerprint entries — `(type id, type name, fingerprint)` — used
/// by the dev-reload graph cache to log which beans changed.
#[cfg(feature = "dev-reload")]
pub type BeanFingerprints = Vec<(TypeId, &'static str, u64)>;

pub trait PostConstruct: Clone + Send + Sync + 'static {
    fn post_construct(&self) -> crate::lifecycle::LifecycleFuture<'_>;
}

/// Unified registration entry point for beans, async beans, and producers.
///
/// Implemented automatically by `#[bean]`, `#[derive(Bean)]`, and `#[producer]`
/// as an inherent per-type impl (never a blanket impl, to avoid overlap). It
/// lets [`AppBuilder::register`](crate::AppBuilder::register) register any of
/// the three registration kinds through a single method:
///
/// - `#[bean]` (sync) / `#[derive(Bean)]` → `Provided = Self`,
///   `Deps = <Self as Bean>::Deps`.
/// - `#[bean]` (async) → `Provided = Self`, `Deps = <Self as AsyncBean>::Deps`.
/// - `#[producer]` → `Provided = <Self as Producer>::Output`,
///   `Deps = <Self as Producer>::Deps`.
pub trait Registrable {
    /// The type made available in the [`BeanContext`] once registered.
    ///
    /// For beans this is `Self`; for producers it is the producer's `Output`.
    /// Tracked in the builder's compile-time provision list.
    type Provided: Clone + Send + Sync + 'static;

    /// The type-level list of dependency types required to construct the value.
    type Deps;

    /// Register this type into the given [`BeanRegistry`].
    fn register_into(registry: &mut BeanRegistry);
}

// ── BeanContext ─────────────────────────────────────────────────────────────

/// Read-only container holding all resolved bean instances.
///
/// Produced by [`BeanRegistry::resolve`]. Each entry is keyed by [`TypeId`].
///
/// Internally uses a two-layer design: a shared `Arc` base (which lazy bean
/// factories can cheaply snapshot) plus an overlay for newly added entries.
/// This avoids `Arc::try_unwrap` failures when lazy factories hold snapshots.
pub struct BeanContext {
    /// Shared base entries. May be referenced by lazy bean factories.
    base: Arc<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    /// Overlay: entries added after the base was created. Checked first by `get()`.
    overlay: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    /// Lazy bean slots: initialized on first `get::<T>()`.
    /// Shared via `Arc` so clones (used by lazy factory snapshots) see
    /// already-resolved values from the same `OnceLock` instances.
    lazy_slots: Arc<RwLock<HashMap<TypeId, Arc<dyn crate::lazy::LazyResolve>>>>,
}

impl Clone for BeanContext {
    fn clone(&self) -> Self {
        Self {
            base: Arc::clone(&self.base),
            // Lazy snapshots don't need the overlay — they only depend on
            // beans that were already constructed (i.e., in the base).
            // But to keep Clone simple, we share the base and start a
            // fresh overlay. This is only used by lazy factories.
            overlay: HashMap::new(),
            // Share the same lazy slots so all clones see resolved values.
            lazy_slots: Arc::clone(&self.lazy_slots),
        }
    }
}

impl fmt::Debug for BeanContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lazy_count = self.lazy_slots.read().map(|m| m.len()).unwrap_or(0);
        f.debug_struct("BeanContext")
            .field("base_count", &self.base.len())
            .field("overlay_count", &self.overlay.len())
            .field("lazy_count", &lazy_count)
            .finish()
    }
}

impl BeanContext {
    /// Create an empty context (no beans).
    ///
    /// Used as the placeholder before graph resolution and for the
    /// [`with_state`](crate::AppBuilder::with_state) path, which bypasses the
    /// bean graph entirely.
    pub fn empty() -> Self {
        Self::new(HashMap::new())
    }

    /// Create a new BeanContext wrapping the given entries as the shared base.
    fn new(entries: HashMap<TypeId, Box<dyn Any + Send + Sync>>) -> Self {
        Self {
            base: Arc::new(entries),
            overlay: HashMap::new(),
            lazy_slots: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Attach lazy bean slots to this context.
    fn with_lazy_slots(
        mut self,
        slots: Arc<RwLock<HashMap<TypeId, Arc<dyn crate::lazy::LazyResolve>>>>,
    ) -> Self {
        self.lazy_slots = slots;
        self
    }

    /// Insert a new entry, creating a new context that shares the same base.
    ///
    /// If the base `Arc` has no other references, the new entry is merged
    /// into the base directly (zero overhead). Otherwise the entry goes
    /// into the overlay (which is checked first by `get()`).
    fn with_new_entry(
        mut self,
        type_id: TypeId,
        value: Box<dyn Any + Send + Sync>,
    ) -> Self {
        // Fast path: if we're the sole owner of the base, merge everything
        // into a single HashMap for the next iteration.
        if let Some(base) = Arc::get_mut(&mut self.base) {
            // Drain overlay into base first
            for (k, v) in self.overlay.drain() {
                base.insert(k, v);
            }
            base.insert(type_id, value);
        } else {
            // A lazy factory holds a snapshot of the base. New entries
            // go into the overlay.
            self.overlay.insert(type_id, value);
        }
        self
    }

    /// Retrieve a bean by type, cloning it out of the context.
    ///
    /// Checks the overlay first, then the shared base. If the bean is not
    /// found eagerly, checks the lazy slots and constructs it on first access.
    ///
    /// # Panics
    ///
    /// Panics if the requested type was not registered or provided.
    pub fn get<T: Clone + 'static>(&self) -> T {
        self.try_get::<T>().unwrap_or_else(|| {
            panic!(
                "Bean of type `{}` not found in context",
                type_name::<T>()
            )
        })
    }

    /// Try to retrieve a bean by type, returning `None` if absent.
    pub fn try_get<T: Clone + 'static>(&self) -> Option<T> {
        let tid = TypeId::of::<T>();
        // Fast path: eagerly-constructed bean (overlay → base)
        if let Some(val) = self
            .overlay
            .get(&tid)
            .or_else(|| self.base.get(&tid))
            .and_then(|v| v.downcast_ref::<T>())
        {
            return Some(val.clone());
        }
        // Lazy path: construct on first access
        let slot = self
            .lazy_slots
            .read()
            .ok()
            .and_then(|slots| slots.get(&tid).map(Arc::clone))?;
        let resolved = slot.resolve();
        resolved.downcast_ref::<T>().cloned()
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

/// A post-construct callback that runs after all beans are resolved.
/// Takes ownership of BeanContext and returns it (same pattern as Factory)
/// to avoid lifetime issues with async closures.
type PostConstructFn = Box<
    dyn FnOnce(
            BeanContext,
        ) -> Pin<
            Box<dyn Future<Output = Result<BeanContext, Box<dyn std::error::Error + Send + Sync>>> + Send>,
        > + Send,
>;

/// Registration for a lazy bean: excluded from the topological sort,
/// resolved on first `get::<T>()` call.
struct LazyBeanRegistration {
    type_id: TypeId,
    type_name: &'static str,
    /// (TypeId, human-readable name) for each dependency — used for validation only.
    dependencies: Vec<(TypeId, &'static str)>,
    /// (config_key, expected_type_name) for config validation.
    config_keys: Vec<(&'static str, &'static str)>,
    #[cfg_attr(not(feature = "dev-reload"), allow(dead_code))]
    build_version: u64,
    /// Creates a `LazySlot<T>` (type-erased as `Arc<dyn LazyResolve>`) given a
    /// `BeanContext` snapshot containing the lazy bean's dependencies.
    slot_factory: Box<dyn FnOnce(BeanContext) -> Arc<dyn crate::lazy::LazyResolve> + Send>,
    /// When `true`, this registration can be replaced by a later registration
    /// of the same `TypeId`.
    #[allow(dead_code)]
    overridable: bool,
}

#[cfg(feature = "dev-reload")]
struct FingerprintReg<'a> {
    type_id: TypeId,
    type_name: &'static str,
    dependencies: &'a Vec<(TypeId, &'static str)>,
    config_keys: &'a Vec<(&'static str, &'static str)>,
    build_version: u64,
}

/// Builder that collects bean registrations and provided instances,
/// resolves the dependency graph, and produces a [`BeanContext`].
#[doc(hidden)]
pub struct BeanRegistry {
    beans: Vec<BeanRegistration>,
    lazy_beans: Vec<LazyBeanRegistration>,
    provided: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    /// Types whose provided instance is **pinned**: any later `provide` /
    /// `register` of the same `TypeId` is silently ignored. Used by test
    /// harnesses that pre-configure the builder *before* handing it to the
    /// application's assembly function (see `AppBuilder::override_bean`).
    pinned: HashSet<TypeId>,
}

struct BeanRegistration {
    type_id: TypeId,
    type_name: &'static str,
    /// (TypeId, human-readable name) for each dependency.
    dependencies: Vec<(TypeId, &'static str)>,
    /// (config_key, expected_type_name) for config validation.
    config_keys: Vec<(&'static str, &'static str)>,
    /// Hash of the constructor/producer source tokens, computed at compile time.
    /// Changes when the bean's code is modified. Used by the dev-reload
    /// fingerprinting system.
    #[cfg_attr(not(feature = "dev-reload"), allow(dead_code))]
    build_version: u64,
    factory: Factory,
    /// Optional post-construct callback, set via `register_post_construct`.
    post_construct: Option<PostConstructFn>,
    /// When `true`, this registration can be replaced by a later registration
    /// of the same `TypeId` (used by the default/alternative bean pattern).
    overridable: bool,
}

/// Read-only view shared by eager ([`BeanRegistration`]) and lazy
/// ([`LazyBeanRegistration`]) registrations so that deduplication, alternative
/// resolution, and topological sorting are written once instead of being
/// duplicated per registration kind.
trait RegMeta {
    fn reg_type_id(&self) -> TypeId;
    fn reg_type_name(&self) -> &'static str;
    fn reg_dependencies(&self) -> &[(TypeId, &'static str)];
    /// Whether a later registration of the same `TypeId` may supersede this
    /// one. Sorting-only views (e.g. fingerprint snapshots) return `false`.
    fn reg_overridable(&self) -> bool;
}

impl RegMeta for BeanRegistration {
    fn reg_type_id(&self) -> TypeId {
        self.type_id
    }
    fn reg_type_name(&self) -> &'static str {
        self.type_name
    }
    fn reg_dependencies(&self) -> &[(TypeId, &'static str)] {
        &self.dependencies
    }
    fn reg_overridable(&self) -> bool {
        self.overridable
    }
}

impl RegMeta for LazyBeanRegistration {
    fn reg_type_id(&self) -> TypeId {
        self.type_id
    }
    fn reg_type_name(&self) -> &'static str {
        self.type_name
    }
    fn reg_dependencies(&self) -> &[(TypeId, &'static str)] {
        &self.dependencies
    }
    fn reg_overridable(&self) -> bool {
        self.overridable
    }
}

#[cfg(feature = "dev-reload")]
impl RegMeta for FingerprintReg<'_> {
    fn reg_type_id(&self) -> TypeId {
        self.type_id
    }
    fn reg_type_name(&self) -> &'static str {
        self.type_name
    }
    fn reg_dependencies(&self) -> &[(TypeId, &'static str)] {
        self.dependencies.as_slice()
    }
    // Fingerprint snapshots are built after dedup; ordering never consults this.
    fn reg_overridable(&self) -> bool {
        false
    }
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
    /// A post-construct hook failed.
    PostConstruct(String),
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
        }
    }
}

impl std::error::Error for BeanError {}

impl BeanRegistry {
    /// Create a new, empty registry.
    pub fn new() -> Self {
        Self {
            beans: Vec::new(),
            lazy_beans: Vec::new(),
            provided: HashMap::new(),
            pinned: HashSet::new(),
        }
    }

    /// Provide a pre-built instance (e.g. external types like `SqlitePool`).
    ///
    /// The instance will be available to beans that depend on type `T`.
    pub fn provide<T: Clone + Send + Sync + 'static>(&mut self, value: T) -> &mut Self {
        if self.pinned.contains(&TypeId::of::<T>()) {
            return self;
        }
        self.provided.insert(TypeId::of::<T>(), Box::new(value));
        self
    }

    /// Provide a **pinned** instance: any later `provide`/`register` of the
    /// same type is silently ignored, so this value wins even over
    /// registrations made after it.
    ///
    /// This is the test-override primitive: a harness pins its mocks and test
    /// doubles before handing the builder to the application's assembly
    /// function, whose own registrations of the same types are then no-ops.
    pub fn pin_provide<T: Clone + Send + Sync + 'static>(&mut self, value: T) -> &mut Self {
        self.provided.insert(TypeId::of::<T>(), Box::new(value));
        self.pinned.insert(TypeId::of::<T>());
        self
    }

    /// Get a reference to a previously provided instance.
    pub fn get_provided<T: Clone + 'static>(&self) -> Option<&T> {
        self.provided
            .get(&TypeId::of::<T>())
            .and_then(|v| v.downcast_ref::<T>())
    }

    /// Returns `true` if a bean (eager or lazy) of type `T` is registered
    /// (via `register`) but not yet materialized.
    ///
    /// Used by plugin dependency resolution to produce a clear error when
    /// a plugin asks for a bean that exists only as a registration at
    /// plugin-install time (before the bean graph is built).
    #[doc(hidden)]
    pub fn is_bean_registered(&self, tid: TypeId) -> bool {
        self.beans.iter().any(|r| r.type_id == tid)
            || self.lazy_beans.iter().any(|r| r.type_id == tid)
    }

    /// Register a (sync) bean type for automatic construction.
    ///
    /// The bean's dependencies will be resolved from other beans or provided
    /// instances during [`resolve`](Self::resolve).
    pub fn register<T: Bean>(&mut self) -> &mut Self {
        self.register_inner::<T>(false)
    }

    /// Register a default (sync) bean that can be overridden by an alternative.
    ///
    /// Same as [`register`](Self::register) but marks the registration as
    /// overridable: a later registration of the same `TypeId` will silently
    /// replace it (used by the default/alternative bean pattern).
    pub fn register_default<T: Bean>(&mut self) -> &mut Self {
        self.register_inner::<T>(true)
    }

    fn register_inner<T: Bean>(&mut self, overridable: bool) -> &mut Self {
        if self.pinned.contains(&TypeId::of::<T>()) {
            return self;
        }
        if T::LAZY {
            self.lazy_beans.push(LazyBeanRegistration {
                type_id: TypeId::of::<T>(),
                type_name: type_name::<T>(),
                dependencies: T::dependencies(),
                config_keys: T::config_keys(),
                build_version: T::BUILD_VERSION,
                slot_factory: Box::new(|ctx| {
                    Arc::new(crate::lazy::LazySlot::new(move || {
                        Box::pin(async move { T::build(&ctx) })
                    })) as Arc<dyn crate::lazy::LazyResolve>
                }),
                overridable,
            });
        } else {
            self.beans.push(BeanRegistration {
                type_id: TypeId::of::<T>(),
                type_name: type_name::<T>(),
                dependencies: T::dependencies(),
                config_keys: T::config_keys(),
                build_version: T::BUILD_VERSION,
                factory: Box::new(|ctx| {
                    Box::pin(async move {
                        let bean = T::build(&ctx);
                        let boxed: Box<dyn Any + Send + Sync> = Box::new(bean);
                        (ctx, boxed)
                    })
                }),
                post_construct: None,
                overridable,
            });
        }
        T::after_register(self);
        self
    }

    /// Register an async bean type for automatic construction.
    ///
    /// The bean's constructor is awaited during resolution.
    pub fn register_async<T: AsyncBean>(&mut self) -> &mut Self {
        self.register_async_inner::<T>(false)
    }

    /// Register a default async bean that can be overridden by an alternative.
    pub fn register_async_default<T: AsyncBean>(&mut self) -> &mut Self {
        self.register_async_inner::<T>(true)
    }

    fn register_async_inner<T: AsyncBean>(&mut self, overridable: bool) -> &mut Self {
        if self.pinned.contains(&TypeId::of::<T>()) {
            return self;
        }
        if T::LAZY {
            self.lazy_beans.push(LazyBeanRegistration {
                type_id: TypeId::of::<T>(),
                type_name: type_name::<T>(),
                dependencies: T::dependencies(),
                config_keys: T::config_keys(),
                build_version: T::BUILD_VERSION,
                slot_factory: Box::new(|ctx| {
                    Arc::new(crate::lazy::LazySlot::new(move || {
                        Box::pin(async move { T::build(&ctx).await })
                    })) as Arc<dyn crate::lazy::LazyResolve>
                }),
                overridable,
            });
        } else {
            self.beans.push(BeanRegistration {
                type_id: TypeId::of::<T>(),
                type_name: type_name::<T>(),
                dependencies: T::dependencies(),
                config_keys: T::config_keys(),
                build_version: T::BUILD_VERSION,
                factory: Box::new(|ctx| {
                    Box::pin(async move {
                        let bean = T::build(&ctx).await;
                        let boxed: Box<dyn Any + Send + Sync> = Box::new(bean);
                        (ctx, boxed)
                    })
                }),
                post_construct: None,
                overridable,
            });
        }
        T::after_register(self);
        self
    }

    /// Register a post-construct hook for a previously registered bean.
    ///
    /// Finds the last `BeanRegistration` matching `T`'s `TypeId` and attaches
    /// the post-construct callback. Called from generated `after_register`.
    pub fn register_post_construct<T: PostConstruct>(&mut self) {
        let tid = TypeId::of::<T>();
        if let Some(reg) = self.beans.iter_mut().rev().find(|r| r.type_id == tid) {
            reg.post_construct = Some(Box::new(|ctx: BeanContext| {
                Box::pin(async move {
                    let bean: T = ctx.get();
                    bean.post_construct().await?;
                    Ok(ctx)
                })
            }));
        }
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
        if self.pinned.contains(&TypeId::of::<T>()) {
            return;
        }
        // Derive a stable per-registration fingerprint from the closure type's
        // name. The name encodes the closure's definition site, so identical
        // closures at distinct call sites hash to distinct values. This is not
        // perfect — it won't invalidate on config changes the closure reads —
        // but it's strictly better than the previous hard-coded `0`, which
        // collapsed every factory registration into the same fingerprint.
        let build_version = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            type_name::<F>().hash(&mut hasher);
            type_name::<T>().hash(&mut hasher);
            hasher.finish()
        };
        self.beans.push(BeanRegistration {
            type_id: TypeId::of::<T>(),
            type_name: type_name::<T>(),
            dependencies: vec![(
                TypeId::of::<crate::config::R2eConfig>(),
                "R2eConfig",
            )],
            config_keys: vec![],
            build_version,
            factory: Box::new(move |ctx| {
                Box::pin(async move {
                    let config = ctx.get::<crate::config::R2eConfig>();
                    let bean = factory(&config);
                    let boxed: Box<dyn Any + Send + Sync> = Box::new(bean);
                    (ctx, boxed)
                })
            }),
            post_construct: None,
            overridable: false,
        });
    }

    /// Register a producer for automatic construction of its output type.
    ///
    /// The producer is awaited during resolution. The resulting bean is
    /// registered under the producer's `Output` type.
    pub fn register_producer<P: Producer>(&mut self) -> &mut Self {
        self.register_producer_inner::<P>(false)
    }

    /// Register a default producer that can be overridden by an alternative.
    pub fn register_producer_default<P: Producer>(&mut self) -> &mut Self {
        self.register_producer_inner::<P>(true)
    }

    fn register_producer_inner<P: Producer>(&mut self, overridable: bool) -> &mut Self {
        if self.pinned.contains(&TypeId::of::<P::Output>()) {
            return self;
        }
        self.beans.push(BeanRegistration {
            type_id: TypeId::of::<P::Output>(),
            type_name: type_name::<P::Output>(),
            dependencies: P::dependencies(),
            config_keys: P::config_keys(),
            build_version: P::BUILD_VERSION,
            factory: Box::new(|ctx| {
                Box::pin(async move {
                    let output = P::produce(&ctx).await;
                    let boxed: Box<dyn Any + Send + Sync> = Box::new(output);
                    (ctx, boxed)
                })
            }),
            post_construct: None,
            overridable,
        });
        self
    }

    /// Compute the graph fingerprint without constructing any beans.
    ///
    /// Performs alternative resolution, topological sorting, and computes
    /// per-bean fingerprints from metadata only. This is cheap
    /// and allows `build_state` to compare against the cached fingerprint
    /// before doing the expensive construction step.
    ///
    /// **Note:** This does NOT validate missing dependencies or config keys.
    /// Validation happens in [`resolve()`](Self::resolve) which is called when
    /// the fingerprint changes and a full rebuild is needed.
    ///
    /// Returns `(graph_fingerprint, per_bean_fingerprints)`.
    #[cfg(feature = "dev-reload")]
    pub fn compute_fingerprint(&self) -> Result<(u64, BeanFingerprints), BeanError> {
        // Work on a snapshot of bean metadata to handle deduplication
        // without mutating self (resolve() will do the real dedup later).
        let alt_remove = Self::overridable_indices_to_remove(&self.beans);
        let lazy_alt_remove = Self::overridable_indices_to_remove(&self.lazy_beans);

        let mut beans: Vec<FingerprintReg<'_>> = self.beans.iter().enumerate()
            .filter(|(i, _)| !alt_remove.contains(i))
            .map(|(_, reg)| FingerprintReg {
                type_id: reg.type_id,
                type_name: reg.type_name,
                dependencies: &reg.dependencies,
                config_keys: &reg.config_keys,
                build_version: reg.build_version,
            })
            .collect();

        // Include lazy beans in the fingerprint graph.
        let lazy_regs: Vec<FingerprintReg<'_>> = self.lazy_beans
            .iter()
            .enumerate()
            .filter(|(i, _)| !lazy_alt_remove.contains(i))
            .map(|(_, reg)| FingerprintReg {
                type_id: reg.type_id,
                type_name: reg.type_name,
                dependencies: &reg.dependencies,
                config_keys: &reg.config_keys,
                build_version: reg.build_version,
            })
            .collect();

        beans.extend(lazy_regs);

        let bean_count = beans.len();
        if bean_count == 0 {
            return Ok((0, Vec::new()));
        }

        // Topological sort (shared generic with resolve(); detects cycles).
        let sorted_order = Self::topological_sort(&beans)?;

        // Compute fingerprints — we need config for this
        let config = self.provided
            .get(&TypeId::of::<crate::config::R2eConfig>())
            .and_then(|v| v.downcast_ref::<crate::config::R2eConfig>());

        let mut dep_fingerprints: HashMap<TypeId, u64> = HashMap::new();
        let mut per_bean: BeanFingerprints = Vec::new();
        let mut graph_hasher = std::collections::hash_map::DefaultHasher::new();


        for &idx in &sorted_order {
            let reg = &beans[idx];
            let fp = Self::compute_reg_fingerprint(reg, config, &dep_fingerprints);
            dep_fingerprints.insert(reg.type_id, fp);
            per_bean.push((reg.type_id, reg.type_name, fp));
            fp.hash(&mut graph_hasher);
        }

        Ok((graph_hasher.finish(), per_bean))
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

        // Resolve default/alternative beans: remove overridable registrations
        // that have been superseded by a later registration of the same TypeId.
        Self::resolve_alternatives(&mut self.beans);
        Self::resolve_alternatives(&mut self.lazy_beans);

        let bean_count = self.beans.len();
        let lazy_type_ids: HashSet<TypeId> =
            self.lazy_beans.iter().map(|lr| lr.type_id).collect();

        // Check for duplicates before any construction.
        Self::check_for_duplicates(&self.beans, &entries)?;
        Self::check_for_lazy_duplicates(&self.lazy_beans, &entries, &self.beans)?;

        let mut ctx = if bean_count == 0 {
            BeanContext::new(entries)
        } else {
            // Build dependency graph
            let id_to_idx = Self::build_type_index(&self.beans);

            // Include lazy beans in the known-types set for dependency validation
            Self::check_missing_dependencies(
                &self.beans,
                &entries,
                &id_to_idx,
                &lazy_type_ids,
            )?;

            // Validate config keys before construction
            Self::validate_config_keys(&self.beans, &entries)?;

            // Topological sort (shared generic; builds its own type index).
            let sorted_order = Self::topological_sort(&self.beans)?;

            // Extract post-construct fns before consuming beans
            let pc_fns: Vec<Option<PostConstructFn>> = sorted_order
                .iter()
                .map(|&idx| self.beans[idx].post_construct.take())
                .collect();

            // Construct beans in order (async)
            let mut ctx =
                Self::construct_beans_in_order(self.beans, sorted_order, entries).await;

            // Run post-construct hooks in topological order
            for pc_fn in pc_fns.into_iter().flatten() {
                ctx = pc_fn(ctx)
                    .await
                    .map_err(|e| BeanError::PostConstruct(e.to_string()))?;
            }

            ctx
        };

        // ── Lazy beans ──────────────────────────────────────────────────
        if !self.lazy_beans.is_empty() {
            // Validate lazy bean dependencies: all deps must exist in the
            // eagerly-resolved set, provided instances, or other lazy beans.
            let eager_ids: HashSet<TypeId> = ctx
                .base
                .keys()
                .chain(ctx.overlay.keys())
                .copied()
                .collect();

            for lazy_reg in &self.lazy_beans {
                for (dep_id, dep_name) in &lazy_reg.dependencies {
                    if !eager_ids.contains(dep_id) && !lazy_type_ids.contains(dep_id) {
                        return Err(BeanError::MissingDependency {
                            bean: lazy_reg.type_name.to_string(),
                            dependency: dep_name.to_string(),
                        });
                    }
                }
            }

            // Validate lazy bean config keys
            let lazy_keys: Vec<_> = self
                .lazy_beans
                .iter()
                .flat_map(|reg| {
                    reg.config_keys
                        .iter()
                        .map(move |(key, ty_name)| (reg.type_name, *key, *ty_name))
                })
                .collect();
            Self::do_validate_config_keys(
                &lazy_keys,
                ctx.try_get::<crate::config::R2eConfig>().as_ref(),
            )?;

            // Build lazy slots from the fully resolved context.
            // Use a shared, mutable map so snapshots can resolve lazy-to-lazy deps.
            let lazy_slots: Arc<RwLock<HashMap<TypeId, Arc<dyn crate::lazy::LazyResolve>>>> =
                Arc::new(RwLock::new(HashMap::new()));
            ctx = ctx.with_lazy_slots(Arc::clone(&lazy_slots));
            for lazy_reg in self.lazy_beans {
                let snapshot = ctx.clone();
                let slot = (lazy_reg.slot_factory)(snapshot);
                lazy_slots
                    .write()
                    .expect("Lazy slots lock poisoned")
                    .insert(lazy_reg.type_id, slot);
            }
        }

        Ok(ctx)
    }

    /// Shared config-key validation: checks the given triples against an R2eConfig.
    fn do_validate_config_keys(
        all_keys: &[(&str, &str, &str)],
        config: Option<&crate::config::R2eConfig>,
    ) -> Result<(), BeanError> {
        if all_keys.is_empty() {
            return Ok(());
        }
        let Some(config) = config else {
            return Ok(());
        };
        let errors = crate::config::validate_keys(config, all_keys);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(BeanError::MissingConfigKeys(
                crate::config::ConfigValidationError { errors },
            ))
        }
    }

    /// Compute the set of indices whose registrations are overridable and
    /// have been superseded by a later registration of the same `TypeId`.
    /// Works uniformly for eager and lazy registrations via [`RegMeta`].
    fn overridable_indices_to_remove<R: RegMeta>(regs: &[R]) -> HashSet<usize> {
        if !regs.iter().any(|r| r.reg_overridable()) {
            return HashSet::new();
        }

        let mut type_indices: HashMap<TypeId, Vec<(usize, bool)>> = HashMap::new();
        for (i, reg) in regs.iter().enumerate() {
            type_indices
                .entry(reg.reg_type_id())
                .or_default()
                .push((i, reg.reg_overridable()));
        }

        let mut remove = HashSet::new();
        for indices in type_indices.values() {
            if indices.len() <= 1 {
                continue;
            }
            let last_idx = indices.last().unwrap().0;
            for &(idx, overridable) in indices {
                if idx != last_idx && overridable {
                    remove.insert(idx);
                }
            }
        }
        remove
    }

    /// Remove overridable (default) registrations that have been superseded
    /// by a later (alternative) registration of the same `TypeId`.
    ///
    /// This runs before the global duplicate-check so that the
    /// default/alternative pattern never trips it.
    /// Works uniformly for eager and lazy registrations via [`RegMeta`].
    fn resolve_alternatives<R: RegMeta>(regs: &mut Vec<R>) {
        let remove = Self::overridable_indices_to_remove(regs);
        if !remove.is_empty() {
            let mut idx = 0;
            regs.retain(|_| {
                let keep = !remove.contains(&idx);
                idx += 1;
                keep
            });
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

    /// Check for duplicate lazy registrations, or conflicts with eager beans or provided entries.
    fn check_for_lazy_duplicates(
        lazy_beans: &[LazyBeanRegistration],
        entries: &HashMap<TypeId, Box<dyn Any + Send + Sync>>,
        beans: &[BeanRegistration],
    ) -> Result<(), BeanError> {
        let eager_ids: HashSet<TypeId> = beans.iter().map(|r| r.type_id).collect();
        let mut seen: HashMap<TypeId, &str> = HashMap::new();
        for reg in lazy_beans {
            if entries.contains_key(&reg.type_id) || eager_ids.contains(&reg.type_id) {
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
    /// `lazy_type_ids` contains TypeIds of lazy beans (also considered "known").
    fn check_missing_dependencies(
        beans: &[BeanRegistration],
        entries: &HashMap<TypeId, Box<dyn Any + Send + Sync>>,
        id_to_idx: &HashMap<TypeId, usize>,
        lazy_type_ids: &HashSet<TypeId>,
    ) -> Result<(), BeanError> {
        for reg in beans {
            for (dep_id, dep_name) in &reg.dependencies {
                if !entries.contains_key(dep_id)
                    && !id_to_idx.contains_key(dep_id)
                    && !lazy_type_ids.contains(dep_id)
                {
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

        let config = entries
            .get(&TypeId::of::<crate::config::R2eConfig>())
            .and_then(|v| v.downcast_ref::<crate::config::R2eConfig>());

        Self::do_validate_config_keys(&all_keys, config)
    }

    /// Perform a topological sort (Kahn's algorithm) over any slice of
    /// registrations. Returns construction order, or a [`BeanError::CyclicDependency`]
    /// listing the nodes left in a cycle. Dependencies pointing outside the
    /// slice (provided instances) are ignored for ordering.
    ///
    /// Shared by [`resolve`](Self::resolve) and (under `dev-reload`)
    /// [`compute_fingerprint`](Self::compute_fingerprint) so both stay in lockstep.
    fn topological_sort<R: RegMeta>(nodes: &[R]) -> Result<Vec<usize>, BeanError> {
        let n = nodes.len();
        let id_to_idx: HashMap<TypeId, usize> = nodes
            .iter()
            .enumerate()
            .map(|(i, r)| (r.reg_type_id(), i))
            .collect();

        // in_degree = number of deps that are other nodes (not provided).
        let mut in_degree: Vec<usize> = nodes
            .iter()
            .map(|reg| {
                reg.reg_dependencies()
                    .iter()
                    .filter(|(d, _)| id_to_idx.contains_key(d))
                    .count()
            })
            .collect();

        // Dependents: for each node index, which other node indices depend on it.
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, reg) in nodes.iter().enumerate() {
            for (dep_id, _) in reg.reg_dependencies() {
                if let Some(&dep_idx) = id_to_idx.get(dep_id) {
                    dependents[dep_idx].push(i);
                }
            }
        }

        // Seed queue with nodes whose deps are all already provided.
        let mut queue: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
        let mut sorted_order: Vec<usize> = Vec::with_capacity(n);

        while let Some(idx) = queue.pop() {
            sorted_order.push(idx);
            for &dep_idx in &dependents[idx] {
                in_degree[dep_idx] -= 1;
                if in_degree[dep_idx] == 0 {
                    queue.push(dep_idx);
                }
            }
        }

        // If not all nodes were sorted, there's a cycle. Walk the stuck
        // subgraph (nodes with `in_degree > 0`) to extract one concrete
        // cycle path, so the error reads "A -> B -> C -> A" instead of
        // listing every node tangled in the strongly connected component.
        if sorted_order.len() != n {
            let cycle = Self::find_cycle(nodes, &id_to_idx, &in_degree);
            return Err(BeanError::CyclicDependency { cycle });
        }

        Ok(sorted_order)
    }

    /// Extract one concrete dependency cycle from the subgraph left unsorted
    /// by Kahn's algorithm, as type names ending with a repeat of the first
    /// element (`[A, B, C, A]`).
    ///
    /// After Kahn's algorithm stalls, exactly the unsorted nodes have
    /// `in_degree > 0`, and every cycle lies entirely within them, so the DFS
    /// only follows edges between such nodes. The first back-edge to a node on
    /// the current DFS path closes a cycle.
    fn find_cycle<R: RegMeta>(
        nodes: &[R],
        id_to_idx: &HashMap<TypeId, usize>,
        in_degree: &[usize],
    ) -> Vec<String> {
        // 0 = unvisited, 1 = on the current DFS path, 2 = fully explored.
        const ON_PATH: u8 = 1;
        const DONE: u8 = 2;

        fn dfs<R: RegMeta>(
            i: usize,
            nodes: &[R],
            id_to_idx: &HashMap<TypeId, usize>,
            in_degree: &[usize],
            color: &mut [u8],
            path: &mut Vec<usize>,
        ) -> Option<Vec<usize>> {
            color[i] = ON_PATH;
            path.push(i);
            for (dep_id, _) in nodes[i].reg_dependencies() {
                let Some(&j) = id_to_idx.get(dep_id) else { continue };
                if in_degree[j] == 0 {
                    continue; // sorted node — cannot be part of a cycle
                }
                match color[j] {
                    ON_PATH => {
                        let start = path.iter().position(|&x| x == j).unwrap();
                        let mut cycle = path[start..].to_vec();
                        cycle.push(j);
                        return Some(cycle);
                    }
                    DONE => {}
                    _ => {
                        if let Some(cycle) = dfs(j, nodes, id_to_idx, in_degree, color, path) {
                            return Some(cycle);
                        }
                    }
                }
            }
            path.pop();
            color[i] = DONE;
            None
        }

        let mut color = vec![0u8; nodes.len()];
        let mut path = Vec::new();
        for i in 0..nodes.len() {
            if in_degree[i] > 0 && color[i] == 0 {
                if let Some(cycle) = dfs(i, nodes, id_to_idx, in_degree, &mut color, &mut path) {
                    return cycle
                        .into_iter()
                        .map(|idx| nodes[idx].reg_type_name().to_string())
                        .collect();
                }
            }
        }

        // Unreachable when called after a stalled Kahn sort, but degrade
        // gracefully: report the stuck nodes as before.
        (0..nodes.len())
            .filter(|&i| in_degree[i] > 0)
            .map(|i| nodes[i].reg_type_name().to_string())
            .collect()
    }

    /// Compute a full fingerprint for a bean, incorporating its own config
    /// fingerprint, its `BUILD_VERSION`, and the fingerprints of all its
    /// dependencies (transitively).
    #[cfg(feature = "dev-reload")]
    fn compute_reg_fingerprint(
        reg: &FingerprintReg<'_>,
        config: Option<&crate::config::R2eConfig>,
        dep_fingerprints: &HashMap<TypeId, u64>,
    ) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();

        // 1. Build version (hash of constructor source tokens)
        reg.build_version.hash(&mut hasher);

        // 2. Config values this bean depends on
        if !reg.config_keys.is_empty() {
            if let Some(config) = config {
                let keys: Vec<&str> = reg.config_keys.iter().map(|(k, _)| *k).collect();
                config.config_fingerprint(&keys).hash(&mut hasher);
            }
        }

        // 3. Fingerprints of all bean dependencies (transitively propagated)
        for (dep_id, _) in reg.dependencies {
            if let Some(&dep_fp) = dep_fingerprints.get(dep_id) {
                dep_fp.hash(&mut hasher);
            }
        }

        hasher.finish()
    }

    /// Construct beans in topological order (async).
    ///
    /// Factories receive a `BeanContext` (entries behind `Arc`) and return it.
    /// Lazy bean factories may clone the context to capture a dependency
    /// snapshot. When that happens, `Arc::get_mut` fails and new entries go
    /// into the overlay. This two-layer design avoids the `Arc::try_unwrap`
    /// panic that would otherwise occur.
    async fn construct_beans_in_order(
        beans: Vec<BeanRegistration>,
        sorted_order: Vec<usize>,
        entries: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    ) -> BeanContext {
        let mut bean_data: Vec<Option<(TypeId, Factory)>> = beans
            .into_iter()
            .map(|r| Some((r.type_id, r.factory)))
            .collect();

        let mut ctx = BeanContext::new(entries);

        for idx in sorted_order {
            let (type_id, factory) = bean_data[idx].take().unwrap();
            let (returned_ctx, bean_value) = factory(ctx).await;
            ctx = returned_ctx.with_new_entry(type_id, bean_value);
        }

        ctx
    }
}

impl Default for BeanRegistry {
    fn default() -> Self {
        Self::new()
    }
}
