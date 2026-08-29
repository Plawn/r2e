//! Controller registration extension traits.
//!
//! Registration must infer two witnesses per controller — `W` (the extraction
//! markers of a state-generic controller impl) and `DepIdx` (the index
//! witnesses proving the controller's `Deps` are present in the state). A
//! plain inherent method would force call sites to write
//! `register_controller::<C, _, _>()`, because Rust's turbofish cannot supply
//! a prefix of a function's generic arguments.
//!
//! Instead, the witnesses live on these **traits** (blanket-implemented for
//! every typed `AppBuilder<T>`) while the controller type stays on the
//! method — the same pattern as
//! [`BeanAccess::get`](crate::type_list::BeanAccess::get) — so call sites
//! read:
//!
//! ```ignore
//! app.register_controller::<UserController>()
//!    .register_controllers::<(AccountController, DataController)>()
//! ```
//!
//! Both traits are exported from the prelude.

use super::*;
use crate::type_list::ControllerTuple;

/// Registers a [`FeatureModule`] on the NoState builder, inferring the
/// encapsulation-check witnesses.
///
/// One call installs the plugins the module brings, registers the module's
/// providers, queues its controllers for registration at `build_state()`, and
/// applies the closed-subgraph encapsulation checks at compile time:
///
/// - every provider dependency must be another provider's output or a
///   declared import (`Deps ⊆ Provided ∪ Imports`);
/// - every controller `#[inject]` dependency must be in the same module
///   scope;
/// - `Exports ⊆ Provided` — a module cannot export what it does not provide;
/// - every plugin in `RequiredPlugins` must already be installed (by the app,
///   an earlier module, or this module's own `Plugins`).
///
/// The plugins in [`Plugins`](FeatureModule::Plugins) are installed **first**,
/// exactly as `.plugin(..)` at this position would: their provisions join the
/// app-global `P` (so they need no `exports(..)` entry, and must not appear in
/// one), their `Deps` join `R`, their controllers are queued, and their
/// effects apply at this `register_module` call's position in install order.
/// A plugin installed twice (app + module, or two modules) is a
/// [`DuplicatePlugin`](crate::beans::BeanError::DuplicatePlugin) boot error
/// naming both owners.
///
/// Only `Exports` join the app-global provision list `P` (the application
/// state); other providers stay private — depending on them from outside the
/// module is a compile error. `Imports` join the requirement list `R`,
/// checked against the final provisions at `build_state()`.
///
/// Note: private providers still live in the global, `TypeId`-keyed graph.
/// Two modules must not each register a private provider of the same
/// concrete type — that is a loud
/// [`DuplicateBean`](crate::beans::BeanError::DuplicateBean) error at
/// startup. Use newtypes for same-shaped private beans.
///
/// ```ignore
/// AppBuilder::new()
///     .provide(db_pool)
///     .register_module::<UserModule>()
///     .register_module::<OrderModule>()
///     .build_state()
///     .await
/// ```
pub trait RegisterModule<P, R, Mods, DepIdx, ExpIdx, CtrlIdx, EndpIdx, PlugIdx>: Sized {
    /// Register a [`FeatureModule`]: its providers, controllers, and
    /// import/export declarations, in one call.
    fn register_module<M>(self) -> ModuleRegistered<M, P, R, Mods>
    where
        M: FeatureModule,
        M::Plugins: ModulePlugins<P, R, Mods>,
        M::Providers: BeanList,
        <M::Providers as BeanList>::Provided: TAppend<ModulePluginProvisions<M>>,
        ModuleProvided<M>: TAppend<M::Imports>,
        M::Controllers: ControllerDepsList,
        <M::Providers as BeanList>::Deps: ModuleDepsSatisfied<ModuleScope<M>, DepIdx>,
        M::Exports: ExportsProvided<<M::Providers as BeanList>::Provided, ExpIdx>,
        <M::Controllers as ControllerDepsList>::Deps: ModuleDepsSatisfied<ModuleScope<M>, CtrlIdx>,
        <M::Endpoints as ModuleEndpointSet>::Deps: ModuleDepsSatisfied<ModuleScope<M>, EndpIdx>,
        M::RequiredPlugins: RequiredPluginsInstalled<ModulePluginsP<M, P, R, Mods>, PlugIdx>,
        M::Exports: TAppend<ModulePluginsP<M, P, R, Mods>>,
        ModulePluginsR<M, P, R, Mods>: TAppend<M::Imports>;
}

impl<P, R, Mods, DepIdx, ExpIdx, CtrlIdx, EndpIdx, PlugIdx>
    RegisterModule<P, R, Mods, DepIdx, ExpIdx, CtrlIdx, EndpIdx, PlugIdx>
    for AppBuilder<NoState, P, R, Mods>
{
    fn register_module<M>(self) -> ModuleRegistered<M, P, R, Mods>
    where
        M: FeatureModule,
        M::Plugins: ModulePlugins<P, R, Mods>,
        M::Providers: BeanList,
        <M::Providers as BeanList>::Provided: TAppend<ModulePluginProvisions<M>>,
        ModuleProvided<M>: TAppend<M::Imports>,
        M::Controllers: ControllerDepsList,
        <M::Providers as BeanList>::Deps: ModuleDepsSatisfied<ModuleScope<M>, DepIdx>,
        M::Exports: ExportsProvided<<M::Providers as BeanList>::Provided, ExpIdx>,
        <M::Controllers as ControllerDepsList>::Deps: ModuleDepsSatisfied<ModuleScope<M>, CtrlIdx>,
        <M::Endpoints as ModuleEndpointSet>::Deps: ModuleDepsSatisfied<ModuleScope<M>, EndpIdx>,
        M::RequiredPlugins: RequiredPluginsInstalled<ModulePluginsP<M, P, R, Mods>, PlugIdx>,
        M::Exports: TAppend<ModulePluginsP<M, P, R, Mods>>,
        ModulePluginsR<M, P, R, Mods>: TAppend<M::Imports>,
    {
        self.register_module_impl::<M, DepIdx, ExpIdx, CtrlIdx, EndpIdx, PlugIdx>()
    }
}

/// Registers a single [`Controller`], inferring its witnesses.
///
/// A controller injecting a bean that is absent from the application state is
/// rejected at compile time here (via the `Deps: AllSatisfied` bound).
pub trait RegisterController<T, W, DepIdx>: Sized
where
    T: Clone + Send + Sync + 'static,
{
    /// Register a [`Controller`] whose routes will be merged into the
    /// application.
    ///
    /// This also collects event consumers and scheduled task definitions
    /// declared on the controller, so that they are started automatically by
    /// `serve()`. The controller core is constructed once, at this call.
    ///
    /// # Panics
    ///
    /// Panics if config keys or sections declared on the controller fail
    /// validation. Use
    /// [`try_register_controller`](Self::try_register_controller) for a
    /// non-panicking alternative.
    fn register_controller<C>(self) -> Self
    where
        C: Controller<T, W>,
        C::Deps: AllSatisfied<T, DepIdx>;

    /// Register a [`Controller`], returning config-validation errors instead
    /// of panicking.
    ///
    /// Behaves exactly like [`register_controller`](Self::register_controller)
    /// on success. On failure, the controller's aggregated
    /// [`ConfigValidationError`](crate::config::ConfigValidationError) is
    /// returned and the builder is consumed (startup wiring cannot proceed
    /// with a misconfigured controller).
    fn try_register_controller<C>(self) -> Result<Self, crate::config::ConfigValidationError>
    where
        C: Controller<T, W>,
        C::Deps: AllSatisfied<T, DepIdx>;
}

impl<T, W, DepIdx> RegisterController<T, W, DepIdx> for AppBuilder<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn register_controller<C>(self) -> Self
    where
        C: Controller<T, W>,
        C::Deps: AllSatisfied<T, DepIdx>,
    {
        self.register_controller_impl::<C, W, DepIdx>()
    }

    fn try_register_controller<C>(self) -> Result<Self, crate::config::ConfigValidationError>
    where
        C: Controller<T, W>,
        C::Deps: AllSatisfied<T, DepIdx>,
    {
        self.try_register_controller_impl::<C, W, DepIdx>()
    }
}

/// Spawns a background [`ServiceComponent`], inferring its dependency
/// witnesses.
///
/// A service whose `from_context` pulls a bean that is absent from the
/// application state is rejected at compile time here (via the
/// `Deps: AllSatisfied` bound) instead of panicking in `ctx.get()` at startup.
pub trait SpawnService<T, DepIdx>: Sized
where
    T: Clone + Send + Sync + 'static,
{
    /// Spawn a background [`ServiceComponent`] that participates in DI.
    ///
    /// The service is constructed from the retained bean graph via
    /// [`ServiceComponent::from_context`] and started in a Tokio task during
    /// `on_start`. A `CancelToken` is provided and cancelled
    /// automatically during shutdown.
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .provide(pool)
    ///     .build_state().await
    ///     .spawn_service::<MetricsExporter>()
    ///     .serve("0.0.0.0:3000").await
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if config keys declared by the service (via
    /// [`ServiceComponent::config_keys`], emitted by
    /// `#[derive(BackgroundService)]`) are missing from the loaded
    /// configuration. Use [`try_spawn_service`](Self::try_spawn_service) for a
    /// non-panicking alternative.
    fn spawn_service<C>(self) -> Self
    where
        C: ServiceComponent,
        C::Deps: AllSatisfied<T, DepIdx>;

    /// Spawn a [`ServiceComponent`], returning config-validation errors
    /// instead of panicking.
    fn try_spawn_service<C>(self) -> Result<Self, crate::config::ConfigValidationError>
    where
        C: ServiceComponent,
        C::Deps: AllSatisfied<T, DepIdx>;
}

impl<T, DepIdx> SpawnService<T, DepIdx> for AppBuilder<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn spawn_service<C>(self) -> Self
    where
        C: ServiceComponent,
        C::Deps: AllSatisfied<T, DepIdx>,
    {
        self.spawn_service_impl::<C>()
    }

    fn try_spawn_service<C>(self) -> Result<Self, crate::config::ConfigValidationError>
    where
        C: ServiceComponent,
        C::Deps: AllSatisfied<T, DepIdx>,
    {
        self.try_spawn_service_impl::<C>()
    }
}

/// Registers a tuple of [`Controller`]s in one call, inferring all witnesses.
pub trait RegisterControllers<T, W>: Sized
where
    T: Clone + Send + Sync + 'static,
{
    /// Register several [`Controller`]s in one call.
    ///
    /// Folds every element of the tuple through the single-controller
    /// registration path, preserving tuple order, so
    /// `register_controllers::<(A, B, C)>()` is equivalent to
    /// `register_controller::<A>().register_controller::<B>().register_controller::<C>()`.
    /// Supports tuples of arity 1..=16; each element must implement
    /// [`Controller`] with its dependencies present in the state, so a
    /// non-controller in the tuple — or a missing bean — is a compile error.
    ///
    /// # Example
    ///
    /// ```ignore
    /// app.register_controllers::<(UserController, AccountController, DataController)>()
    /// ```
    fn register_controllers<Tup>(self) -> Self
    where
        Tup: ControllerTuple<T, W>;
}

impl<T, W> RegisterControllers<T, W> for AppBuilder<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn register_controllers<Tup>(self) -> Self
    where
        Tup: ControllerTuple<T, W>,
    {
        Tup::register_all(self)
    }
}
