use crate::config::ConfigKeyKind;
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

    /// Returns the config keys referenced by this bean as
    /// `(key, type_name, kind)` triples.
    ///
    /// Used by [`BeanRegistry::resolve`] to validate config presence and, under
    /// `dev-reload`, to fingerprint the config values a bean depends on. The
    /// [`ConfigKeyKind`] decides both:
    /// [`Required`](ConfigKeyKind::Required) keys are presence-validated;
    /// `Required`, [`Optional`](ConfigKeyKind::Optional) and
    /// [`Section`](ConfigKeyKind::Section) keys are fingerprinted (editing any
    /// of them rebuilds the bean under `r2e dev`); and
    /// [`Live`](ConfigKeyKind::Live) keys — `#[live_config]` — are neither:
    /// they are pushed into the bean's `LiveConfig` handle instead. A `Section`
    /// entry's key is a dotted **prefix** (`#[config_section(prefix = "…")]`),
    /// so the whole subtree under it is fingerprinted. The default
    /// implementation returns an empty list.
    fn config_keys() -> Vec<(&'static str, &'static str, ConfigKeyKind)> {
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

    /// Returns the config keys referenced by this bean as
    /// `(key, type_name, kind)` triples. Only
    /// [`Required`](ConfigKeyKind::Required) keys are presence-validated;
    /// `Required` + [`Optional`](ConfigKeyKind::Optional) +
    /// [`Section`](ConfigKeyKind::Section) keys are fingerprinted under
    /// `dev-reload` (a `Section` key is a dotted **prefix**, and covers the
    /// whole subtree under it), [`Live`](ConfigKeyKind::Live) keys are not. The
    /// default implementation returns an empty list.
    fn config_keys() -> Vec<(&'static str, &'static str, ConfigKeyKind)> {
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

    /// Returns the config keys referenced by this producer as
    /// `(key, type_name, kind)` triples. Only
    /// [`Required`](ConfigKeyKind::Required) keys are presence-validated;
    /// `Required` + [`Optional`](ConfigKeyKind::Optional) +
    /// [`Section`](ConfigKeyKind::Section) keys are fingerprinted under
    /// `dev-reload` (a `Section` key is a dotted **prefix**, and covers the
    /// whole subtree under it), [`Live`](ConfigKeyKind::Live) keys are not. The
    /// default implementation returns an empty list.
    fn config_keys() -> Vec<(&'static str, &'static str, ConfigKeyKind)> {
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

    /// Called after registration to allow post-processing.
    fn after_register(_registry: &mut BeanRegistry) {}
}

/// Lifecycle hook called after all beans have been constructed.
///
/// Implement this trait (typically via `#[post_construct]` on a `#[bean]`
/// method) to run initialization logic that requires the fully assembled bean.
/// Per-bean fingerprint entries — `(type id, type name, fingerprint)` — used
/// by the dev-reload graph cache to log which beans changed.
#[cfg(feature = "dev-reload")]
pub type BeanFingerprints = Vec<(TypeId, &'static str, u64)>;

/// `Clone` is **not** a supertrait: a bean's post-construct hook is registered
/// via [`BeanRegistry::register_post_construct`] /
/// [`register_provided_post_construct`](BeanRegistry::register_provided_post_construct)
/// (which pull the bean by value from the graph, so they bound `Clone` there),
/// while a controller core — which is not `Clone` — impls this trait too and is
/// run from its own `Arc` at startup.
pub trait PostConstruct: Send + Sync + 'static {
    fn post_construct(&self) -> crate::lifecycle::LifecycleFuture<'_>;
}

/// Disposal hook, the symmetric counterpart of [`PostConstruct`].
///
/// Implement this trait to run cleanup logic (close pools, flush buffers,
/// cancel background work) during the server's graceful-shutdown sequence.
/// A `PreDestroy` hook is invoked against the bean **as it lives in the
/// resolved graph** (override included), and runs as part of the async
/// shutdown phase — see
/// [`AppBuilder::provide_with_pre_destroy`](crate::AppBuilder::provide_with_pre_destroy)
/// and [`PluginSetupContext::run_pre_destroy`](crate::PluginSetupContext::run_pre_destroy)
/// for the invocation order.
pub trait PreDestroy: Clone + Send + Sync + 'static {
    fn pre_destroy(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
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
    /// Pre-destroy disposal hooks, built during [`resolve`](BeanRegistry::resolve)
    /// from the fully resolved graph. Drained by the builder into the async
    /// shutdown phase. Not carried across [`Clone`] (lazy factory snapshots must
    /// not re-run disposal). Behind a `Mutex` so `BeanContext` stays `Sync`
    /// despite the `FnOnce` hooks (which are `Send` but not `Sync`).
    disposers: std::sync::Mutex<Vec<crate::plugin::AsyncShutdownHook>>,
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
            // Disposal hooks are owned by the primary context only.
            disposers: std::sync::Mutex::new(Vec::new()),
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
            disposers: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Drain the pre-destroy disposal hooks built during resolution.
    ///
    /// Called once by the builder to move the disposers into the async
    /// shutdown phase. Returns an empty vec on any context that never carried
    /// disposers (e.g. a lazy-factory snapshot clone or the `with_state` path).
    #[doc(hidden)]
    pub fn take_disposers(&mut self) -> Vec<crate::plugin::AsyncShutdownHook> {
        std::mem::take(&mut *self.disposers.lock().expect("disposers lock poisoned"))
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
    fn with_new_entry(mut self, type_id: TypeId, value: Box<dyn Any + Send + Sync>) -> Self {
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
        self.try_get::<T>()
            .unwrap_or_else(|| panic!("Bean of type `{}` not found in context", type_name::<T>()))
    }

    /// Try to retrieve an **eagerly constructed** bean by type, without
    /// touching the lazy slots. Used by the dev-reload partial rebuild to
    /// clone unchanged instances out of the previous cycle's context —
    /// a lazy bean must never be force-resolved just to be carried over
    /// (its slot `Arc` is reused instead).
    fn try_get_eager<T: Clone + 'static>(&self) -> Option<T> {
        let tid = TypeId::of::<T>();
        self.overlay
            .get(&tid)
            .or_else(|| self.base.get(&tid))
            .and_then(|v| v.downcast_ref::<T>())
            .cloned()
    }

    /// Look up a lazy slot by `TypeId`. Used by the dev-reload partial
    /// rebuild to carry an unchanged lazy bean's slot (and any
    /// already-resolved value inside it) into the next cycle's context.
    fn lazy_slot(&self, tid: TypeId) -> Option<Arc<dyn crate::lazy::LazyResolve>> {
        self.lazy_slots
            .read()
            .ok()
            .and_then(|slots| slots.get(&tid).map(Arc::clone))
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
/// Fallible: a plugin `build` node surfaces its error as
/// [`BeanError::PluginBuild`]; ordinary bean factories are infallible and
/// always return `Ok`.
type Factory = Box<
    dyn FnOnce(
            BeanContext,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<(BeanContext, Box<dyn Any + Send + Sync>), BeanError>>
                    + Send,
            >,
        > + Send,
>;

/// A post-construct callback that runs after all beans are resolved.
/// Takes ownership of BeanContext and returns it (same pattern as Factory)
/// to avoid lifetime issues with async closures.
type PostConstructFn = Box<
    dyn FnOnce(
            BeanContext,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<BeanContext, Box<dyn std::error::Error + Send + Sync>>>
                    + Send,
            >,
        > + Send,
>;

/// A pre-destroy disposer builder: given the fully resolved [`BeanContext`],
/// it reads the target bean (by type, override-aware) and produces the boxed
/// async shutdown hook that will run `PreDestroy::pre_destroy` at shutdown.
type DisposerBuilder = Box<dyn FnOnce(&BeanContext) -> crate::plugin::AsyncShutdownHook + Send>;

/// A scheduled-source hook: reads its target bean by type from the resolved
/// graph (override-aware, like post-construct hooks) and returns the bean's
/// type-erased scheduled task definitions. Drained by `build_state()` into
/// the scheduler's task registry.
type ScheduledSourceHook = Box<dyn FnOnce(&BeanContext) -> Vec<Box<dyn Any + Send>> + Send>;

/// An event-subscriber hook: reads its target bean by type from the resolved
/// graph (override-aware, like post-construct hooks) and returns the bean's
/// [`EventSubscriber::subscribe`](crate::EventSubscriber::subscribe) future.
/// Drained by `build_state()` into the builder's consumer registrations, which
/// run at server startup (`serve` / `build_with_consumers`).
type EventSubscriberHook =
    Box<dyn FnOnce(&BeanContext) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// A lifecycle service hook: reads from the resolved graph and runs until the
/// shutdown token is cancelled.
pub(crate) type ServiceSourceHook = Box<
    dyn FnOnce(
            &BeanContext,
            tokio_util::sync::CancellationToken,
        ) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send,
>;

/// What a registered background service declares about its configuration:
/// `(type id, type name, config keys, typed section validators)`.
///
/// Two lists because they answer different questions: the keys are the flat
/// `(key, type name, kind)` declarations shared with beans, while the sections
/// carry the section's *type* so the missing-key walk can actually run (a
/// `Section` key entry only has the prefix and a type name — see
/// [`SectionValidator`](crate::config::SectionValidator)).
type ServiceConfigDecl = (
    TypeId,
    &'static str,
    Vec<(&'static str, &'static str, ConfigKeyKind)>,
    Vec<crate::config::SectionValidator>,
);

/// A decorator-fill hook: reads its target bean by type from the resolved
/// graph and fills the bean's shared decorator slot with interceptor sets
/// built from that same graph. Run inside [`BeanRegistry::resolve`] after
/// construction but before post-construct hooks, so `#[post_construct]` and
/// direct calls both see a decorated bean.
type DecoFillHook = Box<dyn FnOnce(&BeanContext) + Send>;

/// Registration for a lazy bean: excluded from the topological sort,
/// resolved on first `get::<T>()` call.
struct LazyBeanRegistration {
    type_id: TypeId,
    type_name: &'static str,
    /// (TypeId, human-readable name) for each dependency — used for validation only.
    dependencies: Vec<(TypeId, &'static str)>,
    /// (config_key, expected_type_name, kind) for config validation and
    /// dev-reload fingerprinting. `Optional` keys are fingerprinted but not
    /// presence-validated; `Live` keys are neither (they are pushed).
    config_keys: Vec<(&'static str, &'static str, ConfigKeyKind)>,
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
    config_keys: &'a Vec<(&'static str, &'static str, ConfigKeyKind)>,
    build_version: u64,
    is_lazy: bool,
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
    /// Post-construct hooks for **provided** values (`.provide()` / plugin
    /// `Provided` beans), which have no `BeanRegistration` to hang a hook on.
    /// Each reads its target bean by type from the resolved context (so a
    /// pinned override is honoured) and awaits `PostConstruct::post_construct`.
    /// Run in registration order, **after** all factory-bean post-constructs.
    /// Keyed by the target's `TypeId` so the dev-reload partial rebuild can
    /// skip hooks for provided values pinned from the previous cycle (their
    /// post-construct already ran on that same instance).
    provided_post_constructs: Vec<(TypeId, PostConstructFn)>,
    /// Pre-destroy disposer builders for provided/plugin beans. Materialized
    /// against the resolved graph at the end of `resolve` and carried on the
    /// [`BeanContext`] for the builder to drain into the shutdown sequence.
    disposers: Vec<DisposerBuilder>,
    /// Scheduled-source hooks queued by `after_register` (generated by
    /// `#[bean]` when `#[scheduled]` methods are present). Taken by the
    /// builder before `resolve` and run against the resolved graph — see
    /// [`register_scheduled_source`](Self::register_scheduled_source).
    /// Keyed by the bean `TypeId` (one hook per type — a re-registration of
    /// the same type, e.g. the default/override pattern, must not schedule
    /// its tasks twice); the `&'static str` is the type name, for diagnostics.
    scheduled_sources: Vec<(TypeId, &'static str, ScheduledSourceHook)>,
    /// Event-subscriber hooks queued by `after_register` (generated by
    /// `#[bean]` when `#[consumer]` methods are present). Taken by the builder
    /// before `resolve` and run against the resolved graph — see
    /// [`register_event_subscriber`](Self::register_event_subscriber). Keyed by
    /// the bean `TypeId` (one hook per type — the default/override pattern
    /// registers twice but must subscribe once).
    event_subscribers: Vec<(TypeId, &'static str, EventSubscriberHook)>,
    /// Service hooks queued by `after_register`, usually generated by
    /// `#[producer(start)]`.
    service_sources: Vec<(TypeId, &'static str, ServiceSourceHook)>,
    /// Config declarations of every registered service source.
    /// Kept separate from [`service_sources`](Self::service_sources) because the
    /// hooks are drained by the builder *before* `resolve`, while these keys are
    /// validated **inside** `resolve` alongside the bean keys — a
    /// `#[producer(start)]` service reading a missing `#[config]` key must fail
    /// in the aggregated startup report, not in `ctx.get()` when the task
    /// starts.
    service_config_keys: Vec<ServiceConfigDecl>,
    /// Decorator-fill hooks queued by `after_register` (generated by `#[bean]`
    /// when a `#[scheduled]`/`#[consumer]` method carries `#[intercept]`). Run
    /// inside [`resolve`](Self::resolve) after bean construction and before
    /// post-construct hooks. Keyed by the bean `TypeId` (one hook per type —
    /// the default/override pattern registers twice but must fill once).
    deco_fills: Vec<(TypeId, DecoFillHook)>,
    /// Eager-clone hooks for **provided** values, keyed by `TypeId`. The
    /// dev-reload partial rebuild pins provided instances from the previous
    /// cycle's context (except `R2eConfig`, which is deliberately re-read
    /// per patch) so reused and rebuilt beans keep sharing one instance.
    provided_reuse_clones: HashMap<TypeId, ReuseCloneFn>,
    /// Provided values that are **derived from the config** and therefore
    /// recomputed from scratch by every `load_config`: the `R2eConfig` itself,
    /// the `LiveConfigRegistry`, and every typed `ConfigProperties` /
    /// `#[config(section)]` bean. The dev-reload partial rebuild must NOT pin
    /// these from the previous cycle — doing so froze `#[config(section)]`
    /// values for a whole dev session. Populated by
    /// [`config_derived_scope`](Self::config_derived_scope).
    config_derived: HashSet<TypeId>,
    /// Whether `provide` calls should currently be recorded as config-derived.
    /// Set only for the duration of a [`config_derived_scope`](Self::config_derived_scope).
    in_config_derived_scope: bool,
    /// The deferred-fill handle on the graph this registry will resolve.
    /// Plugin group factories capture clones of it
    /// ([`PluginBuildContext::graph`](crate::plugin::PluginBuildContext::graph));
    /// the builder fills it right after `resolve()` produces the final
    /// `Arc<BeanContext>`. Fresh per registry, so each dev-reload cycle's
    /// plugins see that cycle's graph.
    graph_handle: crate::plugin::GraphHandle,
}

struct BeanRegistration {
    type_id: TypeId,
    type_name: &'static str,
    /// (TypeId, human-readable name) for each dependency.
    dependencies: Vec<(TypeId, &'static str)>,
    /// (config_key, expected_type_name, kind) for config validation and
    /// dev-reload fingerprinting. `Optional` keys are fingerprinted but not
    /// presence-validated; `Live` keys are neither (they are pushed).
    config_keys: Vec<(&'static str, &'static str, ConfigKeyKind)>,
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
    /// Clones this bean's instance out of a previously resolved context.
    /// The dev-reload partial rebuild uses it to carry unchanged instances
    /// across hot-patch cycles instead of re-running the factory.
    reuse_clone: ReuseCloneFn,
    /// When `true`, this registration is never reused across dev-reload
    /// cycles: its factory re-runs every cycle, and its presence forces graph
    /// resolution even on a same-fingerprint cache hit. Plugin group and
    /// projection nodes are volatile — a plugin's `build` carries side
    /// effects (connections, effect registration) that must re-run per cycle,
    /// matching the previous fresh-install-per-cycle semantics.
    #[cfg_attr(not(feature = "dev-reload"), allow(dead_code))]
    volatile: bool,
}

/// Monomorphized eager-clone hook stored per registration (a plain fn
/// pointer — zero-sized, no runtime cost outside dev-reload rebuilds).
type ReuseCloneFn = fn(&BeanContext) -> Option<Box<dyn Any + Send + Sync>>;

fn reuse_clone_of<T: Clone + Send + Sync + 'static>(
    ctx: &BeanContext,
) -> Option<Box<dyn Any + Send + Sync>> {
    ctx.try_get_eager::<T>()
        .map(|b| Box::new(b) as Box<dyn Any + Send + Sync>)
}

/// Reuse stub for volatile registrations (plugin nodes): never reused, so the
/// hook is never consulted — but the field is a plain fn pointer and needs a
/// value. Returning `None` keeps any accidental call safe (treated as "cannot
/// reuse, rebuild").
fn reuse_clone_none(_ctx: &BeanContext) -> Option<Box<dyn Any + Send + Sync>> {
    None
}

/// Instructions for a dev-reload partial rebuild: which beans of the
/// previous cycle's resolved graph may be reused instead of reconstructed.
///
/// Built by `build_state()` when the graph fingerprint changed: a bean whose
/// **per-bean** fingerprint is unchanged (constructor tokens, config values,
/// and every transitive dependency's fingerprint) keeps its instance from
/// `old_ctx`; everything else — and every transitive dependent of a changed
/// bean, whose fingerprint changes by propagation — is rebuilt.
#[doc(hidden)]
pub struct ReusePlan {
    /// The fully resolved context of the previous dev-reload cycle.
    pub old_ctx: Arc<BeanContext>,
    /// `TypeId`s whose per-bean fingerprint is identical to the previous
    /// cycle's. Only these are candidates for instance reuse.
    pub unchanged: HashSet<TypeId>,
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

impl BeanRegistry {
    /// Create a new, empty registry.
    pub fn new() -> Self {
        Self {
            beans: Vec::new(),
            lazy_beans: Vec::new(),
            provided: HashMap::new(),
            pinned: HashSet::new(),
            provided_post_constructs: Vec::new(),
            disposers: Vec::new(),
            scheduled_sources: Vec::new(),
            event_subscribers: Vec::new(),
            service_sources: Vec::new(),
            service_config_keys: Vec::new(),
            deco_fills: Vec::new(),
            provided_reuse_clones: HashMap::new(),
            // Seeded with the two types `load_config` always re-provides, so
            // the never-pin rule holds even for a registry populated by hand
            // (tests, plugins) rather than through `config_derived_scope`.
            config_derived: HashSet::from([
                TypeId::of::<crate::config::R2eConfig>(),
                TypeId::of::<crate::config::LiveConfigRegistry>(),
            ]),
            in_config_derived_scope: false,
            graph_handle: crate::plugin::GraphHandle::new(),
        }
    }

    /// The deferred-fill graph handle tied to this registry. The builder
    /// grabs it before `resolve()` consumes the registry and fills it once
    /// the resolved context is wrapped in its final `Arc`.
    pub fn graph_handle(&self) -> crate::plugin::GraphHandle {
        self.graph_handle.clone()
    }

    /// Whether a provided instance of this `TypeId` is pinned
    /// (see [`pin_provide`](Self::pin_provide)).
    pub fn is_pinned(&self, type_id: &TypeId) -> bool {
        self.pinned.contains(type_id)
    }

    /// Run `f` with every `provide` inside it recorded as **config-derived**.
    ///
    /// Config-derived provided values are exempt from the dev-reload
    /// partial-rebuild pinning: they are rebuilt from the fresh `R2eConfig` by
    /// the next cycle's `load_config`, so pinning the previous cycle's instance
    /// would serve a stale value for the rest of the dev session. `load_config`
    /// wraps the `R2eConfig` + `LiveConfigRegistry` provisions in one, and
    /// `LoadableConfig for T: ConfigProperties` wraps the typed struct plus
    /// every nested `#[config(section)]` child it registers.
    pub fn config_derived_scope(&mut self, f: impl FnOnce(&mut Self)) {
        let previous = std::mem::replace(&mut self.in_config_derived_scope, true);
        f(self);
        self.in_config_derived_scope = previous;
    }

    /// Provide a pre-built instance (e.g. external types like `SqlitePool`).
    ///
    /// The instance will be available to beans that depend on type `T`.
    pub fn provide<T: Clone + Send + Sync + 'static>(&mut self, value: T) -> &mut Self {
        if self.pinned.contains(&TypeId::of::<T>()) {
            return self;
        }
        self.provided.insert(TypeId::of::<T>(), Box::new(value));
        self.provided_reuse_clones
            .insert(TypeId::of::<T>(), reuse_clone_of::<T>);
        if self.in_config_derived_scope {
            self.config_derived.insert(TypeId::of::<T>());
        }
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
        self.provided_reuse_clones
            .insert(TypeId::of::<T>(), reuse_clone_of::<T>);
        self
    }

    /// Whether a same-fingerprint dev-reload cycle must still resolve the
    /// graph instead of returning the monolithic cached state directly.
    ///
    /// Decorator slots are one-shot and must be rebuilt/refilled every cycle.
    /// Pre-destroy hooks are materialized from the fresh registry during
    /// resolution; the cached context no longer owns them after the previous
    /// builder drained them into its shutdown sequence. Volatile registrations
    /// (plugin nodes) must re-run their factories every cycle — their builds
    /// carry side effects (connections, effect registration) the cached state
    /// does not capture.
    #[cfg(feature = "dev-reload")]
    pub(crate) fn requires_resolution_on_cache_hit(&self) -> bool {
        !self.deco_fills.is_empty()
            || !self.disposers.is_empty()
            || self.beans.iter().any(|b| b.volatile)
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
                        Ok((ctx, boxed))
                    })
                }),
                post_construct: None,
                overridable,
                reuse_clone: reuse_clone_of::<T>,
                volatile: false,
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
                        Ok((ctx, boxed))
                    })
                }),
                post_construct: None,
                overridable,
                reuse_clone: reuse_clone_of::<T>,
                volatile: false,
            });
        }
        T::after_register(self);
        self
    }

    /// Register a post-construct hook for a previously registered bean.
    ///
    /// Finds the last `BeanRegistration` matching `T`'s `TypeId` and attaches
    /// the post-construct callback. Called from generated `after_register`.
    pub fn register_post_construct<T: PostConstruct + Clone>(&mut self) {
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

    /// Register a post-construct hook for a **provided** value.
    ///
    /// Unlike [`register_post_construct`](Self::register_post_construct), which
    /// attaches to a factory `BeanRegistration`, this queues a standalone hook
    /// for a value deposited via [`provide`](Self::provide) (or a plugin's
    /// `Provided` tuple). The hook reads `T` from the resolved context by type —
    /// so a pinned override is honoured — and runs during
    /// [`resolve`](Self::resolve), **after** every factory-bean post-construct,
    /// through the same `BeanError::PostConstruct` error path.
    pub fn register_provided_post_construct<T: PostConstruct + Clone>(&mut self) {
        self.provided_post_constructs.push((
            TypeId::of::<T>(),
            Box::new(|ctx: BeanContext| {
                Box::pin(async move {
                    let bean: T = ctx.get();
                    bean.post_construct().await?;
                    Ok(ctx)
                })
            }),
        ));
    }

    /// Register a bean as a scheduled-task source.
    ///
    /// Called from generated `after_register` when a `#[bean]` impl carries
    /// `#[scheduled]` methods. The hook reads the bean by type from the
    /// resolved graph and collects its type-erased task definitions;
    /// `build_state()` drains the hooks via
    /// [`take_scheduled_sources`](Self::take_scheduled_sources) and hands the
    /// tasks to the scheduler's task registry.
    ///
    /// Override semantics match post-construct hooks: an overridden
    /// *dependency* is the instance the tasks capture (the hook resolves by
    /// type from the final graph), while pinning the scheduled bean *itself*
    /// (`override_bean`) skips its registration entirely — `after_register`
    /// never runs, so its tasks are dropped along with the real bean.
    ///
    /// Idempotent per type: re-registering the same bean type (e.g. the
    /// default/override pattern) keeps a single hook — resolve dedups the
    /// registrations to one instance, and its tasks must not be scheduled
    /// twice.
    pub fn register_scheduled_source<T: crate::scheduled_source::ScheduledSource>(&mut self) {
        let tid = TypeId::of::<T>();
        if self.scheduled_sources.iter().any(|(t, _, _)| *t == tid) {
            return;
        }
        self.scheduled_sources.push((
            tid,
            type_name::<T>(),
            Box::new(|ctx: &BeanContext| {
                let bean: T = ctx.get();
                bean.scheduled_tasks_boxed(ctx)
            }),
        ));
    }

    /// Register a bean as an event subscriber.
    ///
    /// Called from generated `after_register` when a `#[bean]` impl carries
    /// `#[consumer]` methods. The hook reads the bean by type from the
    /// resolved graph and returns its
    /// [`EventSubscriber::subscribe`](crate::EventSubscriber::subscribe)
    /// future; `build_state()` drains the hooks via
    /// [`take_event_subscribers`](Self::take_event_subscribers) into the
    /// builder's consumer registrations, which run at server startup
    /// (`serve` / `build_with_consumers`) — the same point controller
    /// `#[consumer]` methods subscribe.
    ///
    /// Override semantics match scheduled sources: an overridden *dependency*
    /// is the instance the consumers capture (the hook resolves by type from
    /// the final graph), while pinning the consumer bean *itself*
    /// (`override_bean`) skips its registration entirely — `after_register`
    /// never runs, so its subscriptions are dropped along with the real bean.
    ///
    /// Idempotent per type: re-registering the same bean type (e.g. the
    /// default/override pattern) keeps a single hook — resolve dedups the
    /// registrations to one instance, and its consumers must not subscribe
    /// twice (every event would be handled twice).
    pub fn register_event_subscriber<T: crate::EventSubscriber>(&mut self) {
        let tid = TypeId::of::<T>();
        if self.event_subscribers.iter().any(|(t, _, _)| *t == tid) {
            return;
        }
        self.event_subscribers.push((
            tid,
            type_name::<T>(),
            Box::new(|ctx: &BeanContext| {
                let bean: T = ctx.get();
                bean.subscribe()
            }),
        ));
    }

    /// Register a resolved bean as a background service.
    ///
    /// The service is constructed from the final [`BeanContext`] and started by
    /// the builder during server startup.
    pub fn register_service_source<T: crate::ServiceComponent>(&mut self) {
        let tid = TypeId::of::<T>();
        if self.service_sources.iter().any(|(t, _, _)| *t == tid) {
            return;
        }
        self.service_sources.push((
            tid,
            type_name::<T>(),
            Box::new(|ctx: &BeanContext, shutdown| {
                let service = T::from_context(ctx);
                Box::pin(service.start(shutdown))
            }),
        ));
        // Declared separately from the hook: the hook is drained before
        // `resolve`, the keys are validated inside it (see
        // [`service_config_keys`](Self::service_config_keys)).
        self.service_config_keys.push((
            tid,
            type_name::<T>(),
            T::config_keys(),
            T::config_sections(),
        ));
    }

    /// Register a bean as a decorator-slot source.
    ///
    /// Called from generated `after_register` when a `#[bean]` impl carries a
    /// `#[scheduled]`/`#[consumer]` method with `#[intercept]`. The hook reads
    /// the bean by type from the resolved graph and calls
    /// [`BeanDecoFill::__r2e_fill_decos`](crate::decorator::BeanDecoFill::__r2e_fill_decos),
    /// which builds every intercepted method's decorator set from the same
    /// graph and fills the bean's shared decorator slot. Because the slot's
    /// `Arc` is shared with every clone already handed out during resolution,
    /// all holders observe the fill.
    ///
    /// Run inside [`resolve`](Self::resolve) **after** construction but
    /// **before** post-construct hooks (and thus before scheduled-source
    /// collection and consumer subscription), so direct calls and
    /// `#[post_construct]` both see a decorated bean.
    ///
    /// Idempotent per type (default/override registers twice, fills once);
    /// pinning the bean itself skips registration, so the slot stays empty and
    /// methods run undecorated — same as a skipped `#[post_construct]`.
    pub fn register_deco_fill<T: crate::decorator::BeanDecoFill + Clone>(&mut self) {
        let tid = TypeId::of::<T>();
        if self.deco_fills.iter().any(|(t, _)| *t == tid) {
            return;
        }
        self.deco_fills.push((
            tid,
            Box::new(|ctx: &BeanContext| {
                let bean: T = ctx.get();
                bean.__r2e_fill_decos(ctx);
            }),
        ));
    }

    /// Drain the scheduled-source hooks queued by
    /// [`register_scheduled_source`](Self::register_scheduled_source).
    /// Returns `(bean type name, hook)` pairs. Builder-internal.
    #[doc(hidden)]
    pub fn take_scheduled_sources(
        &mut self,
    ) -> Vec<(
        &'static str,
        Box<dyn FnOnce(&BeanContext) -> Vec<Box<dyn Any + Send>> + Send>,
    )> {
        std::mem::take(&mut self.scheduled_sources)
            .into_iter()
            .map(|(_, name, hook)| (name, hook))
            .collect()
    }

    /// Drain the event-subscriber hooks queued by
    /// [`register_event_subscriber`](Self::register_event_subscriber).
    /// Returns `(bean type name, hook)` pairs. Builder-internal.
    #[doc(hidden)]
    pub fn take_event_subscribers(
        &mut self,
    ) -> Vec<(
        &'static str,
        Box<dyn FnOnce(&BeanContext) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>,
    )> {
        std::mem::take(&mut self.event_subscribers)
            .into_iter()
            .map(|(_, name, hook)| (name, hook))
            .collect()
    }

    /// Drain background-service hooks queued by [`register_service_source`](Self::register_service_source).
    #[doc(hidden)]
    pub fn take_service_sources(&mut self) -> Vec<(&'static str, ServiceSourceHook)> {
        std::mem::take(&mut self.service_sources)
            .into_iter()
            .map(|(_, name, hook)| (name, hook))
            .collect()
    }

    /// Register a pre-destroy disposal hook for a provided/plugin bean.
    ///
    /// The hook reads `T` from the resolved graph (override-aware) and is run,
    /// as part of the async shutdown phase, in reverse registration order — see
    /// [`AppBuilder::provide_with_pre_destroy`](crate::AppBuilder::provide_with_pre_destroy).
    pub fn register_pre_destroy<T: PreDestroy>(&mut self) {
        self.disposers.push(Box::new(|ctx: &BeanContext| {
            let bean: T = ctx.get();
            Box::new(move || {
                Box::pin(async move { bean.pre_destroy().await })
                    as Pin<Box<dyn Future<Output = ()> + Send>>
            }) as crate::plugin::AsyncShutdownHook
        }));
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
            dependencies: vec![(TypeId::of::<crate::config::R2eConfig>(), "R2eConfig")],
            config_keys: vec![],
            build_version,
            factory: Box::new(move |ctx| {
                Box::pin(async move {
                    let config = ctx.get::<crate::config::R2eConfig>();
                    let bean = factory(&config);
                    let boxed: Box<dyn Any + Send + Sync> = Box::new(bean);
                    Ok((ctx, boxed))
                })
            }),
            post_construct: None,
            overridable: false,
            reuse_clone: reuse_clone_of::<T>,
            volatile: false,
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
                    Ok((ctx, boxed))
                })
            }),
            post_construct: None,
            overridable,
            reuse_clone: reuse_clone_of::<P::Output>,
            volatile: false,
        });
        P::after_register(self);
        self
    }

    /// Register a [`PreStatePlugin`](crate::PreStatePlugin) as bean-graph
    /// nodes: one **group node** running the plugin's `build` (yielding the
    /// whole `Provided` tuple as a hidden `PluginOut<Pl>` bean), plus one
    /// **projection node** per `Provided` element cloning its slot out of the
    /// group. Called by the blanket `RawPreStatePlugin` impl at `.plugin()`
    /// time; the caller has already handled the all-pinned skip.
    ///
    /// Projections register **strict** (`overridable: false`): colliding with
    /// an app `.provide()`/`.register()` of the same type — or installing the
    /// same plugin twice — is a `DuplicateBean` error at `build_state()`. A
    /// type pinned via [`pin_provide`](Self::pin_provide) *before* install
    /// keeps its override (the projection is skipped); the group still runs.
    ///
    /// All plugin nodes are volatile: rebuilt every dev-reload cycle, and
    /// forcing resolution on a same-fingerprint cache hit.
    pub(crate) fn register_plugin_group<Pl: crate::PreStatePlugin>(
        &mut self,
        plugin: Pl,
        effects: crate::plugin::EffectsSlot,
    ) {
        use crate::plugin::{plugin_action_name, PluginOut};
        use crate::type_list::{PluginDeps, PluginProvisions};

        let name = plugin_action_name::<Pl>();
        let graph_handle = self.graph_handle.clone();
        let base_version = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            type_name::<Pl>().hash(&mut hasher);
            hasher.finish() ^ Pl::BUILD_VERSION
        };

        // Group node: deps = the plugin's declared `Deps` (real topo edges).
        // `R2eConfig` needs no edge — `load_config` provides it as a value,
        // available to every factory before construction starts.
        self.beans.push(BeanRegistration {
            type_id: TypeId::of::<PluginOut<Pl>>(),
            type_name: type_name::<PluginOut<Pl>>(),
            dependencies: <Pl::Deps as PluginDeps>::dependencies(),
            config_keys: vec![],
            build_version: base_version,
            factory: Box::new(move |ctx| {
                Box::pin(async move {
                    let config = ctx.try_get::<crate::config::R2eConfig>();
                    let enabled =
                        crate::plugin::plugin_config_enabled(config.as_ref(), Pl::CONFIG_PREFIX);
                    let typed = crate::plugin::load_plugin_config_from::<Pl>(config.as_ref(), name);
                    let deps = <Pl::Deps as PluginDeps>::resolve_from_context(&ctx);
                    let mut bctx =
                        crate::plugin::PluginBuildContext::new(enabled, graph_handle, config);
                    // Fully qualified so a plugin's own inherent `build` method
                    // (e.g. a builder-style `fn build(self)`) can't shadow it.
                    let provided = crate::plugin::PreStatePlugin::build(plugin, deps, typed, &mut bctx)
                        .await
                        .map_err(
                        |source| BeanError::PluginBuild {
                            plugin: name,
                            source,
                        },
                    )?;
                    // The `enabled` decision travels WITH the effects: it was
                    // taken here, from the graph's `R2eConfig`, and the
                    // install-order action must not recompute it from the
                    // builder's own config (they can disagree — a pinned
                    // `R2eConfig` bean).
                    effects.fill(enabled, bctx.into_effects());
                    let boxed: Box<dyn Any + Send + Sync> = Box::new(PluginOut::<Pl>(provided));
                    Ok((ctx, boxed))
                })
            }),
            post_construct: None,
            overridable: false,
            reuse_clone: reuse_clone_none,
            volatile: true,
        });

        // Projection nodes: one per `Provided` element, cloning slot `i` out
        // of the group tuple. Skipped for pinned types (override wins).
        for (i, (tid, tname)) in <Pl::Provided as PluginProvisions>::element_ids()
            .into_iter()
            .enumerate()
        {
            if self.pinned.contains(&tid) {
                continue;
            }
            self.beans.push(BeanRegistration {
                type_id: tid,
                type_name: tname,
                dependencies: vec![(
                    TypeId::of::<PluginOut<Pl>>(),
                    type_name::<PluginOut<Pl>>(),
                )],
                config_keys: vec![],
                build_version: base_version.wrapping_add(1 + i as u64),
                factory: Box::new(move |ctx| {
                    Box::pin(async move {
                        let out = ctx.get::<PluginOut<Pl>>();
                        Ok((ctx, out.0.clone_element(i)))
                    })
                }),
                post_construct: None,
                overridable: false,
                reuse_clone: reuse_clone_none,
                volatile: true,
            });
        }
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

        let mut beans: Vec<FingerprintReg<'_>> = self
            .beans
            .iter()
            .enumerate()
            .filter(|(i, _)| !alt_remove.contains(i))
            .map(|(_, reg)| FingerprintReg {
                type_id: reg.type_id,
                type_name: reg.type_name,
                dependencies: &reg.dependencies,
                config_keys: &reg.config_keys,
                build_version: reg.build_version,
                is_lazy: false,
            })
            .collect();

        // Include lazy beans in the fingerprint graph.
        let lazy_regs: Vec<FingerprintReg<'_>> = self
            .lazy_beans
            .iter()
            .enumerate()
            .filter(|(i, _)| !lazy_alt_remove.contains(i))
            .map(|(_, reg)| FingerprintReg {
                type_id: reg.type_id,
                type_name: reg.type_name,
                dependencies: &reg.dependencies,
                config_keys: &reg.config_keys,
                build_version: reg.build_version,
                is_lazy: true,
            })
            .collect();

        beans.extend(lazy_regs);

        // The config is needed both for per-bean fingerprints and for the
        // whole-config component of the graph fingerprint.
        let config = self
            .provided
            .get(&TypeId::of::<crate::config::R2eConfig>())
            .and_then(|v| v.downcast_ref::<crate::config::R2eConfig>());

        // Seed the graph fingerprint with the ENTIRE config: an edit that no
        // bean declares in `config_keys()` must still invalidate the cached
        // state, or the `R2eConfig` instance inside the cached graph would be
        // served stale. Per-bean fingerprints stay key-scoped, so such an
        // edit rebuilds nothing — the partial-rebuild path just re-provides
        // the fresh config.
        let mut graph_hasher = std::collections::hash_map::DefaultHasher::new();
        match config {
            Some(config) => config.full_fingerprint().hash(&mut graph_hasher),
            None => 0u64.hash(&mut graph_hasher),
        }

        let bean_count = beans.len();
        if bean_count == 0 {
            return Ok((graph_hasher.finish(), Vec::new()));
        }

        // Topological sort (shared generic with resolve(); detects cycles).
        let sorted_order = Self::topological_sort(&beans)?;

        let mut dep_fingerprints: HashMap<TypeId, u64> = HashMap::new();
        let mut per_bean: BeanFingerprints = Vec::new();

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
    pub async fn resolve(self) -> Result<BeanContext, BeanError> {
        self.resolve_reusing(None).await
    }

    /// Resolve the graph, optionally reusing unchanged instances from a
    /// previous dev-reload cycle. [`resolve`](Self::resolve) is the `None`
    /// case; `build_state()` passes `Some` when a hot-patch changed the
    /// graph fingerprint, so only changed beans (and their transitive
    /// dependents) are reconstructed and every other instance — with its
    /// in-memory state — carries over.
    #[doc(hidden)]
    pub async fn resolve_reusing(
        mut self,
        reuse: Option<ReusePlan>,
    ) -> Result<BeanContext, BeanError> {
        // ── Dev-reload partial rebuild: harvest reusable material ───────
        // Beans targeted by a decorator fill are excluded from reuse: their
        // `DecoSlot` is a `OnceLock` already set on the old instance, so a
        // refill against the new graph would silently no-op and leave stale
        // interceptor sets. They rebuild (fresh slot, fresh fill) instead.
        let deco_targets: HashSet<TypeId> = self.deco_fills.iter().map(|(t, _)| *t).collect();
        // A decorator target cannot keep its old OnceLock-backed slot. Any
        // bean that captured that target (directly or transitively) must also
        // rebuild, otherwise the new context would expose a fresh target while
        // a reused dependent still held a clone of the previous instance.
        let mut forced_rebuild = deco_targets.clone();
        // Same argument for volatile registrations (the plugin group node and
        // its per-provision projections): their factories re-run every cycle by
        // design, so the new context exposes a FRESH plugin bean while any
        // dependent reused from cycle N-1 still holds a clone of the previous
        // one — split-brain, e.g. a service holding a `Tenanted<T>` whose
        // `GraphHandle` points at the dropped cycle-N-1 graph (`NoSource` at
        // request time). Seeding them here lets the closure loop below carry
        // the rebuild to every transitive dependent. Cost: those dependents lose
        // their in-memory state on each hot patch, which is the same trade the
        // deco-target rule already makes, and dev-only.
        forced_rebuild.extend(
            self.beans
                .iter()
                .filter(|reg| reg.volatile)
                .map(|reg| reg.type_id),
        );
        loop {
            let mut grew = false;
            for (type_id, dependencies) in self
                .beans
                .iter()
                .map(|reg| (reg.type_id, &reg.dependencies))
                .chain(
                    self.lazy_beans
                        .iter()
                        .map(|reg| (reg.type_id, &reg.dependencies)),
                )
            {
                if !forced_rebuild.contains(&type_id)
                    && dependencies
                        .iter()
                        .any(|(dep_id, _)| forced_rebuild.contains(dep_id))
                {
                    forced_rebuild.insert(type_id);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        let mut reused_instances: HashMap<TypeId, Box<dyn Any + Send + Sync>> = HashMap::new();
        let mut pinned_provided: HashSet<TypeId> = HashSet::new();
        if let Some(plan) = &reuse {
            for reg in &self.beans {
                // Volatile registrations (plugin nodes) are never carried
                // over: their factories re-run every cycle by design.
                if plan.unchanged.contains(&reg.type_id)
                    && !forced_rebuild.contains(&reg.type_id)
                    && !reg.volatile
                {
                    if let Some(inst) = (reg.reuse_clone)(&plan.old_ctx) {
                        reused_instances.insert(reg.type_id, inst);
                    }
                }
            }
            // Pin provided values from the previous cycle so reused and
            // rebuilt beans keep sharing one instance (no split-brain).
            // **Config-derived** values stay fresh: `R2eConfig`, the
            // `LiveConfigRegistry` and every typed `ConfigProperties` /
            // `#[config(section)]` bean are recomputed from the freshly loaded
            // config by this cycle's `load_config`, so the per-patch YAML
            // re-read is deliberate — config edits must apply on the next
            // patch, whatever shape they are read in.
            for (tid, value) in self.provided.iter_mut() {
                if self.config_derived.contains(tid) || forced_rebuild.contains(tid) {
                    continue;
                }
                if let Some(clone_fn) = self.provided_reuse_clones.get(tid) {
                    if let Some(old) = clone_fn(&plan.old_ctx) {
                        *value = old;
                        pinned_provided.insert(*tid);
                    }
                }
            }
            tracing::debug!(
                reused_beans = reused_instances.len(),
                pinned_provided = pinned_provided.len(),
                "dev-reload: partial rebuild — carrying unchanged instances over"
            );
        }

        let mut entries: HashMap<TypeId, Box<dyn Any + Send + Sync>> = HashMap::new();

        // Move provided instances into the resolved set.
        for (tid, value) in self.provided {
            entries.insert(tid, value);
        }

        // Lift the lifecycle hooks out before the bean fields are consumed.
        let provided_post_constructs = std::mem::take(&mut self.provided_post_constructs);
        let disposer_builders = std::mem::take(&mut self.disposers);
        let deco_fills = std::mem::take(&mut self.deco_fills);

        // Resolve default/alternative beans: remove overridable registrations
        // that have been superseded by a later registration of the same TypeId.
        Self::resolve_alternatives(&mut self.beans);
        Self::resolve_alternatives(&mut self.lazy_beans);

        let bean_count = self.beans.len();
        let lazy_type_ids: HashSet<TypeId> = self.lazy_beans.iter().map(|lr| lr.type_id).collect();

        // Check for duplicates before any construction.
        Self::check_for_duplicates(&self.beans, &entries)?;
        Self::check_for_lazy_duplicates(&self.lazy_beans, &entries, &self.beans)?;

        // ── Config validation, aggregated across every declaring host ────
        //
        // Beans declare their keys through `BeanRegistration::config_keys`;
        // background services registered through `#[producer(start)]` declare
        // theirs through `ServiceComponent::config_keys` / `config_sections`
        // (they are constructed from the graph later, at serve time, where a
        // missing key would otherwise be a fail-late panic).
        //
        // Both go into ONE report: an app missing a bean key *and* a service
        // key must not have to fix them one boot at a time. This also runs
        // unconditionally — a pinned-only app (`bean_count == 0`) can still
        // register a `#[producer(start)]` service.
        Self::validate_all_config(&self.beans, &self.service_config_keys, &entries)?;

        // Factory-bean post-construct hooks, in topological order. Populated
        // inside the construction branch, run after the decorator fills below.
        let mut factory_pc_fns: Vec<PostConstructFn> = Vec::new();

        let mut ctx = if bean_count == 0 {
            BeanContext::new(entries)
        } else {
            // Build dependency graph
            let id_to_idx = Self::build_type_index(&self.beans);

            // Include lazy beans in the known-types set for dependency validation
            Self::check_missing_dependencies(&self.beans, &entries, &id_to_idx, &lazy_type_ids)?;

            // Topological sort (shared generic; builds its own type index).
            let sorted_order = Self::topological_sort(&self.beans)?;

            // Extract post-construct fns before consuming beans. Reused
            // instances skip theirs: the hook already ran on that same
            // instance in the cycle that built it.
            factory_pc_fns = sorted_order
                .iter()
                .filter_map(|&idx| {
                    if reused_instances.contains_key(&self.beans[idx].type_id) {
                        None
                    } else {
                        self.beans[idx].post_construct.take()
                    }
                })
                .collect();

            // Construct beans in order (async)
            Self::construct_beans_in_order(self.beans, sorted_order, entries, reused_instances)
                .await?
        };

        // Fill bean decorator slots from the fully-resolved graph, BEFORE any
        // post-construct hook — so `#[post_construct]` and any direct call see
        // a decorated bean. The slot's Arc is shared with every clone handed
        // out during construction, so the fill is observed everywhere. Runs
        // unconditionally (a pinned-only app has `bean_count == 0` but may
        // still queue a fill via `override_bean_decorated`).
        for (_, fill) in deco_fills {
            fill(&ctx);
        }

        // Run factory-bean post-construct hooks in topological order.
        for pc_fn in factory_pc_fns {
            ctx = pc_fn(ctx)
                .await
                .map_err(|e| BeanError::PostConstruct(e.to_string()))?;
        }

        // Run post-construct hooks for provided/plugin beans, after every
        // factory-bean post-construct. Reads each target by type from the
        // resolved context (pinned overrides honoured). Values pinned from
        // the previous dev-reload cycle skip theirs — same instance, the
        // hook already ran.
        for (tid, pc_fn) in provided_post_constructs {
            if pinned_provided.contains(&tid) {
                continue;
            }
            ctx = pc_fn(ctx)
                .await
                .map_err(|e| BeanError::PostConstruct(e.to_string()))?;
        }

        // ── Lazy beans ──────────────────────────────────────────────────
        if !self.lazy_beans.is_empty() {
            // Validate lazy bean dependencies: all deps must exist in the
            // eagerly-resolved set, provided instances, or other lazy beans.
            let eager_ids: HashSet<TypeId> =
                ctx.base.keys().chain(ctx.overlay.keys()).copied().collect();

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
                    // Only `Required` keys are presence-validated.
                    reg.config_keys
                        .iter()
                        .filter(|(_, _, kind)| kind.is_required())
                        .map(move |(key, ty_name, _)| (reg.type_name, *key, *ty_name))
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
                // Dev-reload partial rebuild: an unchanged lazy bean keeps
                // its previous slot `Arc` — including any already-resolved
                // value inside it — instead of getting a fresh slot.
                if let Some(plan) = &reuse {
                    if plan.unchanged.contains(&lazy_reg.type_id)
                        && !forced_rebuild.contains(&lazy_reg.type_id)
                    {
                        if let Some(slot) = plan.old_ctx.lazy_slot(lazy_reg.type_id) {
                            lazy_slots
                                .write()
                                .expect("Lazy slots lock poisoned")
                                .insert(lazy_reg.type_id, slot);
                            continue;
                        }
                    }
                }
                let snapshot = ctx.clone();
                let slot = (lazy_reg.slot_factory)(snapshot);
                lazy_slots
                    .write()
                    .expect("Lazy slots lock poisoned")
                    .insert(lazy_reg.type_id, slot);
            }
        }

        // Materialize pre-destroy disposers against the fully resolved graph and
        // stash them on the context. Reversed so disposal runs in reverse
        // registration order (last registered disposes first).
        if !disposer_builders.is_empty() {
            let mut hooks: Vec<crate::plugin::AsyncShutdownHook> =
                disposer_builders.into_iter().map(|d| d(&ctx)).collect();
            hooks.reverse();
            ctx.disposers = std::sync::Mutex::new(hooks);
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

    /// Validate every config declaration reaching this graph — bean keys,
    /// `#[producer(start)]` service keys, and those services' typed
    /// `#[config_section]`s — as ONE aggregated [`BeanError::MissingConfigKeys`].
    ///
    /// Beans and services are validated together on purpose: they fail at the
    /// same moment (graph resolution) for the same reason (a key the app never
    /// set), so splitting them into two early returns only means two boots to
    /// find two typos.
    fn validate_all_config(
        beans: &[BeanRegistration],
        services: &[ServiceConfigDecl],
        entries: &HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    ) -> Result<(), BeanError> {
        // Only `Required` keys are presence-validated — `Optional`
        // (`Option<T>`) keys resolve to `None` when absent, `Section` keys are
        // validated through the type-aware validators below, and `Live`
        // (`#[live_config]`) keys start empty and arrive by push.
        let required = |kind: &ConfigKeyKind| kind.is_required();

        let mut all_keys: Vec<(&str, &str, &str)> = beans
            .iter()
            .flat_map(|reg| {
                reg.config_keys
                    .iter()
                    .filter(|(_, _, kind)| required(kind))
                    .map(move |(key, ty_name, _)| (reg.type_name, *key, *ty_name))
            })
            .collect();

        all_keys.extend(services.iter().flat_map(|(_, type_name, keys, _)| {
            keys.iter()
                .filter(|(_, _, kind)| required(kind))
                .map(move |(key, ty_name, _)| (*type_name, *key, *ty_name))
        }));

        let config = entries
            .get(&TypeId::of::<crate::config::R2eConfig>())
            .and_then(|v| v.downcast_ref::<crate::config::R2eConfig>());

        let Some(config) = config else {
            return Ok(());
        };

        let mut errors = crate::config::validate_keys(config, &all_keys);
        for (_, _, _, sections) in services {
            errors.extend(crate::config::validate_declared_sections(sections, config));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(BeanError::MissingConfigKeys(
                crate::config::ConfigValidationError { errors },
            ))
        }
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
                let Some(&j) = id_to_idx.get(dep_id) else {
                    continue;
                };
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

        // 1b. Registration mode. Switching `#[bean]` to `#[bean(lazy)]`
        // changes graph semantics even when the constructor is unchanged.
        reg.is_lazy.hash(&mut hasher);

        // 2. Config values this bean COPIES.
        //
        // Required, optional and section keys alike are fingerprinted, so
        // editing any of them under `r2e dev` rebuilds the bean and its
        // dependents. `#[live_config]` keys are deliberately excluded: their
        // freshness comes from the registry push, not from a rebuild —
        // fingerprinting them would churn the bean (and drop its in-memory
        // state) on every live edit. Empty lists contribute nothing, so a
        // live-only bean keeps a stable fingerprint across live edits.
        //
        // `Section` entries carry a dotted **prefix** instead of an exact key,
        // so they are hashed separately: the bean copied a whole subtree, and
        // must be rebuilt when any key under it moves.
        if let Some(config) = config {
            let mut exact: Vec<&str> = Vec::new();
            let mut prefixes: Vec<&str> = Vec::new();
            for (key, _, kind) in reg.config_keys.iter() {
                if !kind.is_fingerprinted() {
                    continue;
                }
                if kind.is_prefix() {
                    prefixes.push(key);
                } else {
                    exact.push(key);
                }
            }
            if !exact.is_empty() {
                config.config_fingerprint(&exact).hash(&mut hasher);
            }
            if !prefixes.is_empty() {
                prefixes.sort_unstable();
                prefixes.dedup();
                for prefix in prefixes {
                    config.prefix_fingerprint(prefix).hash(&mut hasher);
                }
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
        mut reused_instances: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    ) -> Result<BeanContext, BeanError> {
        let mut bean_data: Vec<Option<(TypeId, Factory)>> = beans
            .into_iter()
            .map(|r| Some((r.type_id, r.factory)))
            .collect();

        let mut ctx = BeanContext::new(entries);

        for idx in sorted_order {
            let (type_id, factory) = bean_data[idx].take().unwrap();
            // Dev-reload partial rebuild: an unchanged bean's instance is
            // inserted at its topological position (dependents constructed
            // later read it from the context) — its factory never runs.
            if let Some(inst) = reused_instances.remove(&type_id) {
                ctx = ctx.with_new_entry(type_id, inst);
                continue;
            }
            let (returned_ctx, bean_value) = factory(ctx).await?;
            ctx = returned_ctx.with_new_entry(type_id, bean_value);
        }

        Ok(ctx)
    }
}

impl Default for BeanRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, feature = "dev-reload"))]
mod fingerprint_tests {
    use super::*;

    #[test]
    fn eager_and_lazy_registration_modes_have_distinct_fingerprints() {
        let dependencies = Vec::new();
        let config_keys = Vec::new();
        let eager = FingerprintReg {
            type_id: TypeId::of::<u32>(),
            type_name: "u32",
            dependencies: &dependencies,
            config_keys: &config_keys,
            build_version: 7,
            is_lazy: false,
        };
        let lazy = FingerprintReg {
            is_lazy: true,
            ..eager
        };

        assert_ne!(
            BeanRegistry::compute_reg_fingerprint(&eager, None, &HashMap::new()),
            BeanRegistry::compute_reg_fingerprint(&lazy, None, &HashMap::new()),
        );
    }
}
