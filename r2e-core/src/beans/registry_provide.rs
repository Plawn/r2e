use super::{
    reuse_clone_of, AsyncBean, Bean, BeanContext, BeanRegistration, BeanRegistry,
    LazyBeanRegistration, PostConstruct, PreDestroy, Producer, ServiceSourceHook,
};
use std::any::{type_name, Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

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
}
