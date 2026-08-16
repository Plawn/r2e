use super::{BeanContext, BeanError, ReuseCloneFn};
use crate::config::ConfigKeyKind;
use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

// ── BeanRegistry ────────────────────────────────────────────────────────────

/// Async factory: takes BeanContext by value (to avoid lifetime issues with
/// async captures), returns the context back along with the constructed bean.
/// Fallible: a plugin `build` node surfaces its error as
/// [`BeanError::PluginBuild`]; ordinary bean factories are infallible and
/// always return `Ok`.
pub(super) type Factory = Box<
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
pub(super) type PostConstructFn = Box<
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
pub(super) type DisposerBuilder = Box<dyn FnOnce(&BeanContext) -> crate::plugin::AsyncShutdownHook + Send>;

/// A scheduled-source hook: reads its target bean by type from the resolved
/// graph (override-aware, like post-construct hooks) and returns the bean's
/// type-erased scheduled task definitions. Drained by `build_state()` into
/// the scheduler's task registry.
pub(super) type ScheduledSourceHook = Box<dyn FnOnce(&BeanContext) -> Vec<Box<dyn Any + Send>> + Send>;

/// An event-subscriber hook: reads its target bean by type from the resolved
/// graph (override-aware, like post-construct hooks) and returns the bean's
/// [`EventSubscriber::subscribe`](crate::EventSubscriber::subscribe) future.
/// Drained by `build_state()` into the builder's consumer registrations, which
/// run at server startup (`serve` / `build_with_consumers`).
pub(super) type EventSubscriberHook =
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
pub(super) type ServiceConfigDecl = (
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
pub(super) type DecoFillHook = Box<dyn FnOnce(&BeanContext) + Send>;

/// Registration for a lazy bean: excluded from the topological sort,
/// resolved on first `get::<T>()` call.
pub(super) struct LazyBeanRegistration {
    pub(super) type_id: TypeId,
    pub(super) type_name: &'static str,
    /// (TypeId, human-readable name) for each dependency — used for validation only.
    pub(super) dependencies: Vec<(TypeId, &'static str)>,
    /// (config_key, expected_type_name, kind) for config validation and
    /// dev-reload fingerprinting. `Optional` keys are fingerprinted but not
    /// presence-validated; `Live` keys are neither (they are pushed).
    pub(super) config_keys: Vec<(&'static str, &'static str, ConfigKeyKind)>,
    #[cfg_attr(not(feature = "dev-reload"), allow(dead_code))]
    pub(super) build_version: u64,
    /// Creates a `LazySlot<T>` (type-erased as `Arc<dyn LazyResolve>`) given a
    /// `BeanContext` snapshot containing the lazy bean's dependencies.
    pub(super) slot_factory: Box<dyn FnOnce(BeanContext) -> Arc<dyn crate::lazy::LazyResolve> + Send>,
    /// When `true`, this registration can be replaced by a later registration
    /// of the same `TypeId`.
    #[allow(dead_code)]
    pub(super) overridable: bool,
}


/// Builder that collects bean registrations and provided instances,
/// resolves the dependency graph, and produces a [`BeanContext`].
#[doc(hidden)]
pub struct BeanRegistry {
    pub(super) beans: Vec<BeanRegistration>,
    pub(super) lazy_beans: Vec<LazyBeanRegistration>,
    pub(super) provided: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    /// Types whose provided instance is **pinned**: any later `provide` /
    /// `register` of the same `TypeId` is silently ignored. Used by test
    /// harnesses that pre-configure the builder *before* handing it to the
    /// application's assembly function (see `AppBuilder::override_bean`).
    pub(super) pinned: HashSet<TypeId>,
    /// Post-construct hooks for **provided** values (`.provide()` / plugin
    /// `Provided` beans), which have no `BeanRegistration` to hang a hook on.
    /// Each reads its target bean by type from the resolved context (so a
    /// pinned override is honoured) and awaits `PostConstruct::post_construct`.
    /// Run in registration order, **after** all factory-bean post-constructs.
    /// Keyed by the target's `TypeId` so the dev-reload partial rebuild can
    /// skip hooks for provided values pinned from the previous cycle (their
    /// post-construct already ran on that same instance).
    pub(super) provided_post_constructs: Vec<(TypeId, PostConstructFn)>,
    /// Pre-destroy disposer builders for provided/plugin beans. Materialized
    /// against the resolved graph at the end of `resolve` and carried on the
    /// [`BeanContext`] for the builder to drain into the shutdown sequence.
    pub(super) disposers: Vec<DisposerBuilder>,
    /// Scheduled-source hooks queued by `after_register` (generated by
    /// `#[bean]` when `#[scheduled]` methods are present). Taken by the
    /// builder before `resolve` and run against the resolved graph — see
    /// [`register_scheduled_source`](Self::register_scheduled_source).
    /// Keyed by the bean `TypeId` (one hook per type — a re-registration of
    /// the same type, e.g. the default/override pattern, must not schedule
    /// its tasks twice); the `&'static str` is the type name, for diagnostics.
    pub(super) scheduled_sources: Vec<(TypeId, &'static str, ScheduledSourceHook)>,
    /// Event-subscriber hooks queued by `after_register` (generated by
    /// `#[bean]` when `#[consumer]` methods are present). Taken by the builder
    /// before `resolve` and run against the resolved graph — see
    /// [`register_event_subscriber`](Self::register_event_subscriber). Keyed by
    /// the bean `TypeId` (one hook per type — the default/override pattern
    /// registers twice but must subscribe once).
    pub(super) event_subscribers: Vec<(TypeId, &'static str, EventSubscriberHook)>,
    /// Service hooks queued by `after_register`, usually generated by
    /// `#[producer(start)]`.
    pub(super) service_sources: Vec<(TypeId, &'static str, ServiceSourceHook)>,
    /// Config declarations of every registered service source.
    /// Kept separate from [`service_sources`](Self::service_sources) because the
    /// hooks are drained by the builder *before* `resolve`, while these keys are
    /// validated **inside** `resolve` alongside the bean keys — a
    /// `#[producer(start)]` service reading a missing `#[config]` key must fail
    /// in the aggregated startup report, not in `ctx.get()` when the task
    /// starts.
    pub(super) service_config_keys: Vec<ServiceConfigDecl>,
    /// Decorator-fill hooks queued by `after_register` (generated by `#[bean]`
    /// when a `#[scheduled]`/`#[consumer]` method carries `#[intercept]`). Run
    /// inside [`resolve`](Self::resolve) after bean construction and before
    /// post-construct hooks. Keyed by the bean `TypeId` (one hook per type —
    /// the default/override pattern registers twice but must fill once).
    pub(super) deco_fills: Vec<(TypeId, DecoFillHook)>,
    /// Eager-clone hooks for **provided** values, keyed by `TypeId`. The
    /// dev-reload partial rebuild pins provided instances from the previous
    /// cycle's context (except `R2eConfig`, which is deliberately re-read
    /// per patch) so reused and rebuilt beans keep sharing one instance.
    pub(super) provided_reuse_clones: HashMap<TypeId, ReuseCloneFn>,
    /// Provided values that are **derived from the config** and therefore
    /// recomputed from scratch by every `load_config`: the `R2eConfig` itself,
    /// the `LiveConfigRegistry`, and every typed `ConfigProperties` /
    /// `#[config(section)]` bean. The dev-reload partial rebuild must NOT pin
    /// these from the previous cycle — doing so froze `#[config(section)]`
    /// values for a whole dev session. Populated by
    /// [`config_derived_scope`](Self::config_derived_scope).
    pub(super) config_derived: HashSet<TypeId>,
    /// Whether `provide` calls should currently be recorded as config-derived.
    /// Set only for the duration of a [`config_derived_scope`](Self::config_derived_scope).
    pub(super) in_config_derived_scope: bool,
    /// The deferred-fill handle on the graph this registry will resolve.
    /// Plugin group factories capture clones of it
    /// ([`PluginBuildContext::graph`](crate::plugin::PluginBuildContext::graph));
    /// the builder fills it right after `resolve()` produces the final
    /// `Arc<BeanContext>`. Fresh per registry, so each dev-reload cycle's
    /// plugins see that cycle's graph.
    pub(super) graph_handle: crate::plugin::GraphHandle,
}

pub(super) struct BeanRegistration {
    pub(super) type_id: TypeId,
    pub(super) type_name: &'static str,
    /// (TypeId, human-readable name) for each dependency.
    pub(super) dependencies: Vec<(TypeId, &'static str)>,
    /// (config_key, expected_type_name, kind) for config validation and
    /// dev-reload fingerprinting. `Optional` keys are fingerprinted but not
    /// presence-validated; `Live` keys are neither (they are pushed).
    pub(super) config_keys: Vec<(&'static str, &'static str, ConfigKeyKind)>,
    /// Hash of the constructor/producer source tokens, computed at compile time.
    /// Changes when the bean's code is modified. Used by the dev-reload
    /// fingerprinting system.
    #[cfg_attr(not(feature = "dev-reload"), allow(dead_code))]
    pub(super) build_version: u64,
    pub(super) factory: Factory,
    /// Optional post-construct callback, set via `register_post_construct`.
    pub(super) post_construct: Option<PostConstructFn>,
    /// When `true`, this registration can be replaced by a later registration
    /// of the same `TypeId` (used by the default/alternative bean pattern).
    pub(super) overridable: bool,
    /// Clones this bean's instance out of a previously resolved context.
    /// The dev-reload partial rebuild uses it to carry unchanged instances
    /// across hot-patch cycles instead of re-running the factory.
    pub(super) reuse_clone: ReuseCloneFn,
    /// When `true`, this registration is never reused across dev-reload
    /// cycles: its factory re-runs every cycle, and its presence forces graph
    /// resolution even on a same-fingerprint cache hit. Plugin group and
    /// projection nodes are volatile — a plugin's `build` carries side
    /// effects (connections, effect registration) that must re-run per cycle,
    /// matching the previous fresh-install-per-cycle semantics.
    #[cfg_attr(not(feature = "dev-reload"), allow(dead_code))]
    pub(super) volatile: bool,
}

/// Read-only view shared by eager ([`BeanRegistration`]) and lazy
/// ([`LazyBeanRegistration`]) registrations so that deduplication, alternative
/// resolution, and topological sorting are written once instead of being
/// duplicated per registration kind.
pub(super) trait RegMeta {
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

impl Default for BeanRegistry {
    fn default() -> Self {
        Self::new()
    }
}
