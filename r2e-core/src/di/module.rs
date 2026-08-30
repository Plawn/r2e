//! Feature modules — closed subgraphs with compile-time encapsulation.
//!
//! A [`FeatureModule`] bundles **providers (beans/producers) + controllers +
//! non-HTTP endpoints (gRPC services) + imports/exports + the plugins it
//! brings** into one unit registered with a single call:
//! [`AppBuilder::register_module::<M>()`](crate::AppBuilder::register_module).
//!
//! Unlike Spring/NestJS modules, encapsulation is enforced **at compile
//! time** at the `register_module` call site:
//!
//! - a module's providers and controllers may depend only on the module's own
//!   provided types plus its declared [`Imports`](FeatureModule::Imports);
//! - only [`Exports`](FeatureModule::Exports) become visible to the app-global
//!   provision list `P` (and therefore to the application state and to other
//!   modules); non-exported providers stay private;
//! - exporting a type the module does not provide is rejected.
//!
//! Everything is derived from the declaration — a module has no `register`
//! body, so an impl cannot misdeclare its dependencies. The `#[module]`
//! attribute macro generates the impl from a provider/controller listing.
//!
//! # Runtime model
//!
//! All providers (private ones included) are registered into the single
//! global [`BeanRegistry`] and constructed by the one topological sort at
//! `build_state()`. Encapsulation controls compile-time **visibility**, not
//! runtime keying: two modules must not each register a *private* provider of
//! the **same concrete type** — the graph is keyed by `TypeId`, so this is a
//! loud [`DuplicateBean`](crate::beans::BeanError::DuplicateBean) error at
//! startup. Use newtypes for same-shaped private beans.
//!
//! Module controllers are registered by `build_state()` right after the state
//! is materialized (their dependency check already happened module-locally at
//! `register_module`), constructing their cores from the retained
//! [`BeanContext`](crate::beans::BeanContext) — where private beans exist.
//!
//! # Modules and other transports
//!
//! A module's [`Endpoints`](FeatureModule::Endpoints) carry the same story for
//! non-HTTP transports: `#[module(grpc_services(GreeterService))]` makes the
//! gRPC service part of the vertical slice, dependency-checked against
//! [`ModuleScope`] at `register_module` and registered by `build_state()` from
//! the same retained bean context — so it may inject the module's **private**
//! providers. r2e-core stays transport-agnostic: it only names the
//! [`ModuleEndpointSet`] / [`ModuleEndpoints`] pair, implemented by the
//! transport crate (`r2e_grpc::ModuleGrpcServices`).
//!
//! # Modules and plugins
//!
//! A module may **require** a plugin ([`RequiredPlugins`](FeatureModule::RequiredPlugins)
//! — "the app must install it before me") or **bring** it
//! ([`Plugins`](FeatureModule::Plugins) — "registering me installs it"). A
//! plugin has exactly **one owner**: the app or one module. Installing the same
//! plugin twice (app + module, or two modules) is a
//! [`DuplicatePlugin`](crate::beans::BeanError::DuplicatePlugin) boot error
//! naming both owners; the fix is `requires_plugins` in every module but the
//! owner.
//!
//! Unlike providers, a brought plugin's beans are **not** module-private: the
//! plugin is installed on the app builder exactly as `.plugin(..)` would (its
//! provisions join the app-global `P`, its `Deps` join `R`, its controllers
//! join the deferred-controller list, its effects apply at the
//! `register_module` position in install order). They therefore need no
//! `exports(..)` entry — and must not appear in one, since `Exports` is
//! checked against the module's own providers.

use crate::beans::{BeanRegistry, Registrable};
use crate::builder::{AppBuilder, NoState};
use crate::controller::{Controller, EndpointDeps};
use crate::plugin::PluginInstall;
use crate::type_list::{Here, TAppend, TCons, TNil, There};

/// A feature module: a closed subgraph of providers + controllers with
/// declared imports and exports.
///
/// Purely declarative — registration, dependency lists, and the
/// encapsulation checks are all derived from the four associated types by
/// [`AppBuilder::register_module`](crate::AppBuilder::register_module).
/// Implement it by hand or generate it with `#[module]`:
///
/// ```ignore
/// struct UserModule;
///
/// impl FeatureModule for UserModule {
///     type Providers = TCons<UserRepo, TCons<UserService, TNil>>;
///     type Controllers = (UserController,);
///     type Exports = TCons<UserService, TNil>;   // UserRepo stays private
///     type Imports = TCons<DbPool, TNil>;        // supplied by the app
///     type RequiredPlugins = ();                 // no plugin required
///     type Plugins = ();                         // no plugin brought
///     type Endpoints = ();                       // no gRPC service
///     fn plugins() {}
/// }
///
/// AppBuilder::new()
///     .provide(db_pool)
///     .register_module::<UserModule>()
///     .build_state()
///     .await
/// ```
pub trait FeatureModule {
    /// Type-level list ([`TCons`]/[`TNil`]) of the module's provider types.
    ///
    /// Each element must implement [`Registrable`] (emitted by `#[bean]`,
    /// `#[derive(Bean)]`, and `#[producer]`). For producers, the element is
    /// the producer struct; the *provided* type is its `Output`.
    type Providers;

    /// Tuple of controller types registered by this module (or `()`).
    ///
    /// Controllers may inject any of the module's provided types (exported or
    /// private) and any import; their routes/consumers/scheduled tasks are
    /// wired automatically when `build_state()` runs.
    type Controllers;

    /// Type-level list of **bean types** (⊆ the providers' provided types)
    /// made visible outside the module.
    ///
    /// Only these join the app-global provision list `P` — i.e. the
    /// application state and other modules' imports. Everything else the
    /// module provides stays private.
    ///
    /// Note the asymmetry for **request-scoped extraction**: `#[inject]`
    /// fields resolve from the retained bean context, so a *private*
    /// provider can back them — but bean-backed request extractors (e.g. an
    /// identity type whose `FromRequestPartsVia` impl has a `HasBean` bound)
    /// resolve from the application **state** `P`. A bean backing such an
    /// extractor must therefore be exported (or imported/provided at app
    /// level); a private one fails the `HasBean` bound at `build_state()`.
    type Exports;

    /// Type-level list of bean types the module requires from outside
    /// (satisfied by the app's `.provide`/`.register` or by another module's
    /// exports).
    ///
    /// Appended to the global requirement list `R` and checked against the
    /// final provision list at `build_state()`.
    type Imports;

    /// **Tuple** of plugin types this module requires (or `()` for
    /// none) — e.g. `(Scheduler,)`.
    ///
    /// Unlike [`Imports`](Self::Imports), which names individual bean types,
    /// this names whole plugins. At `register_module` the compiler verifies
    /// that **every provided bean** of each listed plugin is already in the
    /// app-global provision list `P` — i.e. the plugin was `.plugin(..)`-ed
    /// *before* this module. A missing plugin is a compile error that names the
    /// plugin and points at `.plugin(..)`, rather than surfacing as an opaque
    /// missing-bean error on one of the plugin's internal handle types.
    ///
    /// Set to `()` when the module needs no plugin. `#[module(requires_plugins(
    /// Scheduler))]` generates this.
    ///
    /// See also [`Plugins`](Self::Plugins) — *bringing* a plugin instead of
    /// requiring one.
    type RequiredPlugins;

    /// **Tuple** of plugin types this module **brings** (installs itself), or
    /// `()` for none — e.g. `(Scheduler,)`.
    ///
    /// The instances come from [`plugins()`](Self::plugins). At
    /// `register_module` each is installed exactly as `.plugin(..)` would
    /// install it, **before** the module's own providers are registered: its
    /// provisions join the app-global provision list `P`, its `Deps` join the
    /// requirement list `R`, its `Controllers` join the deferred-controller
    /// list, and its effects apply at this module's position in plugin install
    /// order.
    ///
    /// A plugin has exactly **one owner**. A module that merely *needs* a
    /// plugin someone else installs declares it in
    /// [`RequiredPlugins`](Self::RequiredPlugins) instead; installing the same
    /// plugin twice is a
    /// [`DuplicatePlugin`](crate::beans::BeanError::DuplicatePlugin) boot error
    /// naming both owners.
    ///
    /// The brought plugins' provided beans are in the module's local resolution
    /// scope ([`ModuleScope`]), so the module's providers and controllers may
    /// depend on them without declaring an import. They are **not** module-
    /// private (they are app-global, like any plugin bean), so they must not be
    /// listed in [`Exports`](Self::Exports).
    ///
    /// `#[module(plugins(Scheduler = Scheduler, Executor = Executor::builder()
    /// .max_concurrent(8).build()))]` generates this and `plugins()`.
    type Plugins: ModulePluginList;

    /// The module's **non-HTTP endpoint set** — the transport endpoints it
    /// owns, exactly as [`Controllers`](Self::Controllers) are the HTTP ones —
    /// or `()` for none.
    ///
    /// r2e-core stays transport-agnostic: it knows only the aggregated
    /// dependency list (checked against [`ModuleScope`] at `register_module`,
    /// like a controller's) and the value-level registration fold
    /// ([`ModuleEndpoints`]), both run by `build_state()` from the retained
    /// [`BeanContext`](crate::beans::BeanContext) — so a module endpoint may
    /// inject the module's **private** providers, just like a module
    /// controller.
    ///
    /// The concrete set type lives in the transport crate:
    /// `#[module(grpc_services(GreeterService))]` generates
    /// `type Endpoints = r2e_grpc::ModuleGrpcServices<(GreeterService,)>;`
    /// and adds `GrpcServer` to [`RequiredPlugins`](Self::RequiredPlugins), so
    /// a module whose transport plugin is missing is a compile error naming
    /// that plugin. That check is a *provision* check (see
    /// [`RequiredPluginInstalled`]) — it is exact for gRPC only because
    /// `GrpcServer`'s provision marker cannot be constructed outside r2e-grpc.
    /// A hand-written `FeatureModule` impl that skips `RequiredPlugins`
    /// altogether is caught at boot with
    /// [`BeanError::MissingTransportPlugin`](crate::beans::BeanError::MissingTransportPlugin),
    /// naming the plugin and the module.
    type Endpoints: ModuleEndpointSet;

    /// The configured plugin **instances** matching [`Plugins`](Self::Plugins),
    /// in the same order. `fn plugins() {}` for `type Plugins = ()`.
    fn plugins() -> Self::Plugins;
}

// ── Module endpoints (non-HTTP transports) ──────────────────────────────────
//
// The generic hook the ticket-989 design hangs on. r2e-grpc depends on
// r2e-core, never the reverse, so r2e-core cannot name a gRPC service: it
// declares the two halves of the contract (type-level deps, value-level
// registration) and the transport crate implements them for its own wrapper
// type. Orphan rules force that wrapper (`ModuleGrpcServices<(S0, S1, ..)>`)
// — a foreign trait cannot be implemented for a bare tuple of type
// parameters.

/// The type-level half of a module's [`Endpoints`](FeatureModule::Endpoints):
/// the aggregated dependency list, used for the module-scope check at
/// `register_module`.
///
/// Implemented by transport crates for their endpoint-set wrapper (e.g.
/// `r2e_grpc::ModuleGrpcServices<(S0, S1)>`), and here for `()` (no
/// endpoints). Independent of the application state type, because
/// `register_module` runs in the `NoState` phase — exactly like
/// [`ControllerDepsList`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid `Endpoints` set for a feature module",
    label = "not a module endpoint set",
    note = "`FeatureModule::Endpoints` must be `()` or a transport crate's endpoint-set type, e.g. `r2e_grpc::ModuleGrpcServices<(MyGrpcService,)>`",
    note = "`#[module(grpc_services(MyGrpcService))]` generates it"
)]
pub trait ModuleEndpointSet {
    /// Concatenation of every endpoint's
    /// [`EndpointDeps::Deps`](crate::controller::EndpointDeps::Deps) — core
    /// `#[inject]` deps plus every decorator site's, so the module-scope check
    /// covers interceptors too.
    type Deps;
}

impl ModuleEndpointSet for () {
    type Deps = TNil;
}

/// The value-level half of a module's [`Endpoints`](FeatureModule::Endpoints):
/// registers them into the typed builder at `build_state()`.
///
/// Like [`ModuleControllers`], registration here is **unchecked** — the
/// dependency check already happened module-locally at `register_module`
/// (against [`ModuleScope`]), so requiring the deps to be in the application
/// state would wrongly reject an endpoint injecting a private module bean.
pub trait ModuleEndpoints<T: Clone + Send + Sync + 'static> {
    /// Register every endpoint in the set, in declaration order.
    ///
    /// `module` is the declaring module's type name, carried only so a
    /// boot-time failure (missing transport plugin, duplicate endpoint) can
    /// name the slice that owns the endpoint instead of the endpoint alone.
    ///
    /// Fallible for the same reason [`ModuleControllers::register_all`] is:
    /// `try_build_state()` must not panic on a declared config key, and a
    /// missing transport plugin is a boot error rather than a panic.
    fn register_all(
        builder: AppBuilder<T>,
        module: &'static str,
    ) -> Result<AppBuilder<T>, crate::beans::BeanError>;
}

impl<T: Clone + Send + Sync + 'static> ModuleEndpoints<T> for () {
    fn register_all(
        builder: AppBuilder<T>,
        _module: &'static str,
    ) -> Result<AppBuilder<T>, crate::beans::BeanError> {
        Ok(builder)
    }
}

// ── Module-brought plugins ─────────────────────────────────────────────────

/// Type-level fold over a module's [`Plugins`](FeatureModule::Plugins) tuple:
/// the concatenation of every plugin's
/// [`Provisions`](crate::plugin::PluginInstall::Provisions).
///
/// Independent of the builder's `P`/`R`/`Mods` (unlike [`ModulePlugins`]), so
/// it can be used in the module-scope and export checks. Implemented for `()`
/// and tuples of arity 1..=8.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid `Plugins` tuple for a feature module",
    label = "not a tuple of plugins",
    note = "`FeatureModule::Plugins` must be `()` or a tuple of at most 8 plugin types, e.g. `(Scheduler,)`",
    note = "`#[module(plugins(Scheduler = Scheduler))]` generates it"
)]
pub trait ModulePluginList {
    /// Concatenation of every listed plugin's provision list.
    type Provisions;
}

impl ModulePluginList for () {
    type Provisions = TNil;
}

macro_rules! impl_module_plugin_list {
    ($P0:ident) => {
        impl<$P0> ModulePluginList for ($P0,)
        where
            $P0: PluginInstall,
            <$P0 as PluginInstall>::Provisions: TAppend<TNil>,
        {
            type Provisions = <<$P0 as PluginInstall>::Provisions as TAppend<TNil>>::Output;
        }
    };
    ($P0:ident, $($Ps:ident),+) => {
        impl<$P0, $($Ps),+> ModulePluginList for ($P0, $($Ps),+)
        where
            $P0: PluginInstall,
            ($($Ps,)+): ModulePluginList,
            <$P0 as PluginInstall>::Provisions:
                TAppend<<($($Ps,)+) as ModulePluginList>::Provisions>,
        {
            type Provisions = <<$P0 as PluginInstall>::Provisions as TAppend<
                <($($Ps,)+) as ModulePluginList>::Provisions,
            >>::Output;
        }
        impl_module_plugin_list!($($Ps),+);
    };
}

impl_module_plugin_list!(P0, P1, P2, P3, P4, P5, P6, P7);

/// Value-level fold that installs a module's
/// [`Plugins`](FeatureModule::Plugins) tuple onto the builder, one
/// [`AppBuilder::plugin`] call per element, in tuple order.
///
/// Parameterised by the builder's current `P`/`R`/`Mods` so the output types
/// are *exactly* the sequential fold `.plugin(a).plugin(b)…` produces — no
/// associativity reasoning the compiler cannot do. Implemented for `()` and
/// tuples of arity 1..=8.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be installed as a feature module's `Plugins`",
    label = "not an installable tuple of plugins",
    note = "every type in `FeatureModule::Plugins` must implement `r2e::Plugin` (tuple arity at most 8)"
)]
pub trait ModulePlugins<P, R, Mods> {
    /// The provision list after installing every plugin in the tuple.
    type OutP;
    /// The requirement list after installing every plugin in the tuple.
    type OutR;
    /// The deferred-controller list after installing every plugin in the tuple.
    type OutMods;

    /// Install every plugin in the tuple, in tuple order.
    fn install_all(
        self,
        app: AppBuilder<NoState, P, R, Mods>,
    ) -> AppBuilder<NoState, Self::OutP, Self::OutR, Self::OutMods>;
}

impl<P, R, Mods> ModulePlugins<P, R, Mods> for () {
    type OutP = P;
    type OutR = R;
    type OutMods = Mods;

    fn install_all(self, app: AppBuilder<NoState, P, R, Mods>) -> AppBuilder<NoState, P, R, Mods> {
        app
    }
}

macro_rules! impl_module_plugins {
    ($P0:ident $p0:ident) => {
        impl<P, R, Mods, $P0> ModulePlugins<P, R, Mods> for ($P0,)
        where
            $P0: PluginInstall,
            P: TAppend<<$P0 as PluginInstall>::Provisions>,
            R: TAppend<<$P0 as PluginInstall>::Required>,
            <$P0 as PluginInstall>::Controllers: PushPluginCtrls<$P0, Mods>,
        {
            type OutP = <P as TAppend<<$P0 as PluginInstall>::Provisions>>::Output;
            type OutR = <R as TAppend<<$P0 as PluginInstall>::Required>>::Output;
            type OutMods =
                <<$P0 as PluginInstall>::Controllers as PushPluginCtrls<$P0, Mods>>::Output;

            fn install_all(
                self,
                app: AppBuilder<NoState, P, R, Mods>,
            ) -> AppBuilder<NoState, Self::OutP, Self::OutR, Self::OutMods> {
                app.plugin(self.0)
            }
        }
    };
    ($P0:ident $p0:ident, $($Ps:ident $ps:ident),+) => {
        impl<P, R, Mods, $P0, $($Ps),+> ModulePlugins<P, R, Mods> for ($P0, $($Ps),+)
        where
            $P0: PluginInstall,
            P: TAppend<<$P0 as PluginInstall>::Provisions>,
            R: TAppend<<$P0 as PluginInstall>::Required>,
            <$P0 as PluginInstall>::Controllers: PushPluginCtrls<$P0, Mods>,
            ($($Ps,)+): ModulePlugins<
                <P as TAppend<<$P0 as PluginInstall>::Provisions>>::Output,
                <R as TAppend<<$P0 as PluginInstall>::Required>>::Output,
                <<$P0 as PluginInstall>::Controllers as PushPluginCtrls<$P0, Mods>>::Output,
            >,
        {
            type OutP = <($($Ps,)+) as ModulePlugins<
                <P as TAppend<<$P0 as PluginInstall>::Provisions>>::Output,
                <R as TAppend<<$P0 as PluginInstall>::Required>>::Output,
                <<$P0 as PluginInstall>::Controllers as PushPluginCtrls<$P0, Mods>>::Output,
            >>::OutP;
            type OutR = <($($Ps,)+) as ModulePlugins<
                <P as TAppend<<$P0 as PluginInstall>::Provisions>>::Output,
                <R as TAppend<<$P0 as PluginInstall>::Required>>::Output,
                <<$P0 as PluginInstall>::Controllers as PushPluginCtrls<$P0, Mods>>::Output,
            >>::OutR;
            type OutMods = <($($Ps,)+) as ModulePlugins<
                <P as TAppend<<$P0 as PluginInstall>::Provisions>>::Output,
                <R as TAppend<<$P0 as PluginInstall>::Required>>::Output,
                <<$P0 as PluginInstall>::Controllers as PushPluginCtrls<$P0, Mods>>::Output,
            >>::OutMods;

            fn install_all(
                self,
                app: AppBuilder<NoState, P, R, Mods>,
            ) -> AppBuilder<NoState, Self::OutP, Self::OutR, Self::OutMods> {
                let ($p0, $($ps,)+) = self;
                <($($Ps,)+) as ModulePlugins<_, _, _>>::install_all(
                    ($($ps,)+),
                    app.plugin($p0),
                )
            }
        }
        impl_module_plugins!($($Ps $ps),+);
    };
}

impl_module_plugins!(P0 p0, P1 p1, P2 p2, P3 p3, P4 p4, P5 p5, P6 p6, P7 p7);

/// Fold over a type-level list of [`Registrable`] provider types.
///
/// Derives, from [`FeatureModule::Providers`]:
/// - [`Provided`](Self::Provided): the list of provided bean types (for
///   beans, the type itself; for producers, the output type);
/// - [`Deps`](Self::Deps): the concatenation of every provider's declared
///   dependency list — the module's internal requirements, checked against
///   the module scope (provided ∪ imports) at `register_module`;
/// - [`register_into`](Self::register_into): registers every provider into
///   the global registry, in declaration order.
pub trait BeanList {
    /// `TCons` list of each provider's [`Registrable::Provided`].
    type Provided;
    /// Concatenation of every provider's [`Registrable::Deps`].
    type Deps;
    /// Register every provider into the registry, preserving list order.
    fn register_into(registry: &mut BeanRegistry);
}

impl BeanList for TNil {
    type Provided = TNil;
    type Deps = TNil;
    fn register_into(_registry: &mut BeanRegistry) {}
}

impl<H: Registrable, T: BeanList> BeanList for TCons<H, T>
where
    H::Deps: TAppend<T::Deps>,
{
    type Provided = TCons<H::Provided, T::Provided>;
    type Deps = <H::Deps as TAppend<T::Deps>>::Output;
    fn register_into(registry: &mut BeanRegistry) {
        H::register_into(registry);
        T::register_into(registry);
    }
}

/// The provisions of the plugins a module [brings](FeatureModule::Plugins).
pub type ModulePluginProvisions<M> =
    <<M as FeatureModule>::Plugins as ModulePluginList>::Provisions;

/// Everything a module makes available on its own: its providers' outputs plus
/// the beans of the plugins it brings.
pub type ModuleProvided<M> = <<<M as FeatureModule>::Providers as BeanList>::Provided as TAppend<
    ModulePluginProvisions<M>,
>>::Output;

/// The module's local resolution scope: everything its providers provide, the
/// beans of the plugins it brings, plus its imports. Provider and controller
/// dependencies must resolve here.
pub type ModuleScope<M> = <ModuleProvided<M> as TAppend<<M as FeatureModule>::Imports>>::Output;

// ── Encapsulation checks ────────────────────────────────────────────────────
//
// Structurally these mirror `Contains`/`AllSatisfied` (type_list.rs), but as
// dedicated traits: the compile errors a user sees on an encapsulation
// violation are the innermost unsatisfied bound, and the `Contains`
// diagnostic ("add `.provide(value)` on the AppBuilder") would point at the
// wrong fix — module violations are fixed by editing the module declaration.

/// Compile-time witness that dependency `H` is inside a module's scope
/// (`Self` — the provided ∪ imported list), located at `Idx`.
#[diagnostic::on_unimplemented(
    message = "`{H}` is not in this feature module's scope",
    label = "the module neither provides nor imports `{H}`",
    note = "a module's providers and controllers may depend only on the module's own provided types plus its declared `Imports`",
    note = "add a provider for `{H}` to the module's `Providers`, or declare it in the module's `Imports`"
)]
pub trait InModuleScope<H, Idx> {}

impl<H, T> InModuleScope<H, Here> for TCons<H, T> {}
impl<H, X, T, I> InModuleScope<H, There<I>> for TCons<X, T> where T: InModuleScope<H, I> {}

/// Compile-time verification that every dependency in `Self` (a provider or
/// controller dependency list) is inside the module scope `Scope`.
///
/// `Indices` is an opaque witness tuple inferred by the compiler.
#[diagnostic::on_unimplemented(
    message = "one or more dependencies are outside this feature module's scope",
    note = "each provider/controller dependency must be provided by the module itself or declared in its `Imports`"
)]
pub trait ModuleDepsSatisfied<Scope, Indices> {}

impl<S> ModuleDepsSatisfied<S, ()> for TNil {}
impl<H, T, S, IH, IT> ModuleDepsSatisfied<S, (IH, IT)> for TCons<H, T>
where
    S: InModuleScope<H, IH>,
    T: ModuleDepsSatisfied<S, IT>,
{
}

/// Compile-time witness that exported type `H` is among a module's provided
/// types (`Self`), located at `Idx`.
#[diagnostic::on_unimplemented(
    message = "`{H}` is exported but not provided by this feature module",
    label = "no provider in the module's `Providers` outputs `{H}`",
    note = "`Exports` must be a subset of the providers' provided types — add a provider for `{H}` or remove it from the module's `Exports`",
    note = "beans of a plugin the module brings via `plugins(..)` are already app-global: drop them from `Exports`"
)]
pub trait ProvidedByModule<H, Idx> {}

impl<H, T> ProvidedByModule<H, Here> for TCons<H, T> {}
impl<H, X, T, I> ProvidedByModule<H, There<I>> for TCons<X, T> where T: ProvidedByModule<H, I> {}

/// Compile-time verification that every type in `Self` (a module's export
/// list) is among the module's provided types `Provided`.
///
/// `Indices` is an opaque witness tuple inferred by the compiler.
#[diagnostic::on_unimplemented(
    message = "one or more exported types are not provided by this feature module",
    note = "`Exports` must be a subset of the providers' provided types"
)]
pub trait ExportsProvided<Provided, Indices> {}

impl<P> ExportsProvided<P, ()> for TNil {}
impl<H, T, P, IH, IT> ExportsProvided<P, (IH, IT)> for TCons<H, T>
where
    P: ProvidedByModule<H, IH>,
    T: ExportsProvided<P, IT>,
{
}

// ── Required-plugin checks ──────────────────────────────────────────────────
//
// A module may name whole plugins in `RequiredPlugins`. At `register_module`
// we verify that every provided bean of each such plugin is already in the
// provision list `P` — i.e. the plugin was installed before the module. The
// diagnostic names the *plugin* (and points at `.plugin(..)`), which is far
// clearer than the opaque missing-bean error a module controller would
// otherwise get on one of the plugin's internal handle types.

/// Compile-time witness that required plugin `Plug` is installed — every bean
/// in its [`PluginInstall::Provisions`](crate::plugin::PluginInstall::Provisions)
/// is present in the provision list `Self` (the app-global `P`).
///
/// The check is on the *provisions*, not on the plugin's identity: hand-writing
/// `.provide(..)` for every type a plugin provides satisfies it without the
/// plugin ever running. That is only reachable when a plugin's provision types
/// are constructible by the caller — a plugin whose provision is an
/// unconstructible marker (as `GrpcServer`'s `GrpcMarker` is, so that
/// `grpc_services(..)` really cannot compile without the plugin) is exact.
/// Transports that also need runtime wiring keep a boot-time backstop:
/// [`BeanError::MissingTransportPlugin`](crate::beans::BeanError::MissingTransportPlugin).
#[diagnostic::on_unimplemented(
    message = "this feature module requires the `{Plug}` plugin, which is not installed before it",
    label = "`{Plug}` must be installed before this module",
    note = "install it with `.plugin({Plug})` *before* `.register_module::<_>()` — a module's `RequiredPlugins` must already be in the provision list `P`"
)]
pub trait RequiredPluginInstalled<Plug, Idx> {}

// `do_not_recommend`: without it, a missing plugin surfaces as the inner
// `AllSatisfied`/`Contains` "type `X` was not provided" error on one of the
// plugin's internal handle types — the where-clause diagnostic wins. Suppressing
// this impl from recommendation makes the compiler report the unsatisfied
// `RequiredPluginInstalled` bound directly, so the plugin-naming message above
// fires.
#[diagnostic::do_not_recommend]
impl<P, Plug, Idx> RequiredPluginInstalled<Plug, Idx> for P
where
    Plug: crate::plugin::PluginInstall,
    Plug::Provisions: crate::type_list::AllSatisfied<P, Idx>,
{
}

/// Compile-time verification that every plugin in `Self` (a module's
/// `RequiredPlugins` tuple) is installed in the provision list `P`.
///
/// `Indices` is an opaque witness tuple inferred by the compiler.
#[diagnostic::on_unimplemented(
    message = "one or more of this feature module's required plugins are not installed",
    note = "each type in the module's `RequiredPlugins` must be `.plugin(..)`-ed before `.register_module`"
)]
pub trait RequiredPluginsInstalled<P, Indices> {}

impl<P> RequiredPluginsInstalled<P, ()> for () {}

macro_rules! impl_required_plugins_installed {
    ($($Plug:ident $Idx:ident),+) => {
        impl<P, $($Plug, $Idx),+> RequiredPluginsInstalled<P, ($($Idx,)+)> for ($($Plug,)+)
        where
            $(P: RequiredPluginInstalled<$Plug, $Idx>,)+
        {
        }
    };
}

impl_required_plugins_installed!(P0 I0);
impl_required_plugins_installed!(P0 I0, P1 I1);
impl_required_plugins_installed!(P0 I0, P1 I1, P2 I2);
impl_required_plugins_installed!(P0 I0, P1 I1, P2 I2, P3 I3);
impl_required_plugins_installed!(P0 I0, P1 I1, P2 I2, P3 I3, P4 I4);
impl_required_plugins_installed!(P0 I0, P1 I1, P2 I2, P3 I3, P4 I4, P5 I5);
impl_required_plugins_installed!(P0 I0, P1 I1, P2 I2, P3 I3, P4 I4, P5 I5, P6 I6);
impl_required_plugins_installed!(P0 I0, P1 I1, P2 I2, P3 I3, P4 I4, P5 I5, P6 I6, P7 I7);

/// Aggregate the state-independent dependency lists
/// ([`EndpointDeps::Deps`]) of a controller tuple.
///
/// This is what lets `register_module` check controller dependencies in the
/// NoState phase, before the state type exists: `EndpointDeps::Deps` is the
/// full list the state-generic `Controller::Deps` resolves to — core
/// `#[inject]` deps plus every guard/interceptor site's `DecoratorSpec::Deps`
/// — so the module-scope check covers decorators too.
/// Implemented for `()` and tuples of arity 1..=16.
pub trait ControllerDepsList {
    /// Concatenation of every controller's `EndpointDeps::Deps`.
    type Deps;
}

impl ControllerDepsList for () {
    type Deps = TNil;
}

macro_rules! impl_controller_deps_list {
    ($C0:ident) => {
        impl<$C0: EndpointDeps> ControllerDepsList for ($C0,)
        where
            $C0::Deps: TAppend<TNil>,
        {
            type Deps = <$C0::Deps as TAppend<TNil>>::Output;
        }
    };
    ($C0:ident, $($Cs:ident),+) => {
        impl<$C0: EndpointDeps, $($Cs: EndpointDeps),+> ControllerDepsList
            for ($C0, $($Cs),+)
        where
            ($($Cs,)+): ControllerDepsList,
            $C0::Deps: TAppend<<($($Cs,)+) as ControllerDepsList>::Deps>,
        {
            type Deps =
                <$C0::Deps as TAppend<<($($Cs,)+) as ControllerDepsList>::Deps>>::Output;
        }
        impl_controller_deps_list!($($Cs),+);
    };
}

impl_controller_deps_list!(C0, C1, C2, C3, C4, C5, C6, C7, C8, C9, C10, C11, C12, C13, C14, C15);

/// Registers a module's controller tuple into a typed builder, **without**
/// the global dependency check.
///
/// Module controllers are dependency-checked module-locally at
/// `register_module` (against provided ∪ imports), so the global
/// `AllSatisfied` bound would wrongly reject controllers injecting private
/// module beans — their cores construct from the retained bean context, where
/// those beans exist. `W` collects one extraction-marker witness per element;
/// it is always inferred. Implemented for `()` and tuples of arity 1..=16.
pub trait ModuleControllers<T: Clone + Send + Sync + 'static, W> {
    /// Register every controller in the tuple, in tuple order.
    ///
    /// Fallible: a controller declaring config keys/sections that do not
    /// validate yields [`BeanError::ControllerConfig`](crate::beans::BeanError::ControllerConfig)
    /// rather than a panic, so `try_build_state()` — which runs this fold —
    /// really is non-panicking.
    fn register_all(builder: AppBuilder<T>) -> Result<AppBuilder<T>, crate::beans::BeanError>;
}

/// Register one deferred controller, mapping its config-validation failure to
/// the boot error channel. Shared by the module and plugin folds; the only
/// difference is that module controllers skip the global dependency check
/// (see [`ModuleControllers`]).
fn register_deferred<T, C, W>(
    builder: AppBuilder<T>,
) -> Result<AppBuilder<T>, crate::beans::BeanError>
where
    T: Clone + Send + Sync + 'static,
    C: Controller<T, W>,
{
    builder
        .try_register_controller_unchecked_impl::<C, W>()
        .map_err(|source| crate::beans::BeanError::ControllerConfig {
            controller: std::any::type_name::<C>(),
            source,
        })
}

impl<T: Clone + Send + Sync + 'static> ModuleControllers<T, ()> for () {
    fn register_all(builder: AppBuilder<T>) -> Result<AppBuilder<T>, crate::beans::BeanError> {
        Ok(builder)
    }
}

macro_rules! impl_module_controllers {
    ($C0:ident $W0:ident) => {
        impl<T, $C0, $W0> ModuleControllers<T, ($W0,)> for ($C0,)
        where
            T: Clone + Send + Sync + 'static,
            $C0: Controller<T, $W0>,
        {
            fn register_all(
                builder: AppBuilder<T>,
            ) -> Result<AppBuilder<T>, crate::beans::BeanError> {
                register_deferred::<T, $C0, $W0>(builder)
            }
        }
    };
    ($C0:ident $W0:ident, $($Cs:ident $Ws:ident),+) => {
        impl<T, $C0, $W0, $($Cs, $Ws),+> ModuleControllers<T, ($W0, $($Ws),+)>
            for ($C0, $($Cs),+)
        where
            T: Clone + Send + Sync + 'static,
            $C0: Controller<T, $W0>,
            $($Cs: Controller<T, $Ws>,)+
        {
            fn register_all(
                builder: AppBuilder<T>,
            ) -> Result<AppBuilder<T>, crate::beans::BeanError> {
                let builder = register_deferred::<T, $C0, $W0>(builder)?;
                $(let builder = register_deferred::<T, $Cs, $Ws>(builder)?;)+
                Ok(builder)
            }
        }
        impl_module_controllers!($($Cs $Ws),+);
    };
}

impl_module_controllers!(
    C0 W0, C1 W1, C2 W2, C3 W3, C4 W4, C5 W5, C6 W6, C7 W7, C8 W8, C9 W9,
    C10 W10, C11 W11, C12 W12, C13 W13, C14 W14, C15 W15
);

/// Fold over the builder's pending-module list (`Mods`), registering each
/// module's controllers into the freshly built typed builder.
///
/// `build_state()` applies this right after materializing the state; user
/// code never names it. `W` nests one witness pair per module.
pub trait ModuleList<T: Clone + Send + Sync + 'static, W> {
    /// Register every pending module's controllers, in registration order.
    ///
    /// Fallible for the same reason [`ModuleControllers::register_all`] is:
    /// `try_build_state()` must not panic on a deferred controller's config.
    fn register_controllers(
        builder: AppBuilder<T>,
    ) -> Result<AppBuilder<T>, crate::beans::BeanError>;
}

impl<T: Clone + Send + Sync + 'static> ModuleList<T, ()> for TNil {
    fn register_controllers(
        builder: AppBuilder<T>,
    ) -> Result<AppBuilder<T>, crate::beans::BeanError> {
        Ok(builder)
    }
}

impl<T, M, Rest, WC, WR> ModuleList<T, (WC, WR)> for TCons<ModEntry<M>, Rest>
where
    T: Clone + Send + Sync + 'static,
    M: FeatureModule,
    M::Controllers: ModuleControllers<T, WC>,
    M::Endpoints: ModuleEndpoints<T>,
    Rest: ModuleList<T, WR>,
{
    fn register_controllers(
        builder: AppBuilder<T>,
    ) -> Result<AppBuilder<T>, crate::beans::BeanError> {
        // `Mods` grows head-first (the most recently registered module is the
        // head), so recurse into the tail first to preserve registration order.
        let builder = Rest::register_controllers(builder)?;
        let builder = <M::Controllers as ModuleControllers<T, WC>>::register_all(builder)?;
        // Non-HTTP endpoints (gRPC services) register after the controllers of
        // the same module, from the same retained bean context.
        <M::Endpoints as ModuleEndpoints<T>>::register_all(builder, std::any::type_name::<M>())
    }
}

impl<T, Pl, Rest, WC, WR> ModuleList<T, (WC, WR)> for TCons<PluginCtrls<Pl>, Rest>
where
    T: Clone + Send + Sync + 'static,
    Pl: crate::plugin::PluginInstall,
    Pl::Controllers: PluginControllerList<Pl, T, WC>,
    Rest: ModuleList<T, WR>,
{
    fn register_controllers(
        builder: AppBuilder<T>,
    ) -> Result<AppBuilder<T>, crate::beans::BeanError> {
        <Pl::Controllers as PluginControllerList<Pl, T, WC>>::register_all(
            Rest::register_controllers(builder)?,
        )
    }
}

// ── Deferred-controller entries ─────────────────────────────────────────────
//
// The builder's `Mods` list carries every controller set whose registration is
// deferred to `build_state()`. Two kinds live there — feature modules and
// plugin controllers — and each needs its own `ModuleList` impl. Rust has no
// negative reasoning, so `TCons<M, _> where M: FeatureModule` and
// `TCons<PluginCtrls<Pl>, _>` would be judged overlapping; both kinds are
// therefore wrapped in a distinct marker so the two impls are structurally
// disjoint.

/// `Mods` entry for a feature module registered with `register_module::<M>()`.
/// Marker only — never constructed.
pub struct ModEntry<M>(std::marker::PhantomData<fn() -> M>);

/// `Mods` entry for the controllers a plugin ships
/// ([`Plugin::Controllers`](crate::plugin::Plugin::Controllers)). Marker only —
/// never constructed. Pushed by [`PushPluginCtrls`] and only when the plugin
/// actually declares controllers, so `.plugin(..)` on a controller-free plugin
/// leaves `Mods` at `TNil` (which is what `with_state` requires).
pub struct PluginCtrls<Pl>(std::marker::PhantomData<fn() -> Pl>);

/// Type-level "push `Pl`'s controllers onto `Mods`, if it has any".
///
/// Implemented on the plugin's `Controllers` **tuple type**: `()` yields
/// `Mods` unchanged, any non-empty tuple yields `TCons<PluginCtrls<Pl>, Mods>`.
/// This keeps `Mods = TNil` — and therefore `with_state` — available for the
/// overwhelming majority of plugins, which ship no controllers.
pub trait PushPluginCtrls<Pl: ?Sized, Mods> {
    /// The resulting pending-controller list.
    type Output;
}

impl<Pl: ?Sized, Mods> PushPluginCtrls<Pl, Mods> for () {
    type Output = Mods;
}

macro_rules! impl_push_plugin_ctrls {
    ($($C:ident),+) => {
        impl<Pl, Mods, $($C),+> PushPluginCtrls<Pl, Mods> for ($($C,)+) {
            type Output = TCons<PluginCtrls<Pl>, Mods>;
        }
    };
}

impl_push_plugin_ctrls!(C0);
impl_push_plugin_ctrls!(C0, C1);
impl_push_plugin_ctrls!(C0, C1, C2);
impl_push_plugin_ctrls!(C0, C1, C2, C3);
impl_push_plugin_ctrls!(C0, C1, C2, C3, C4);
impl_push_plugin_ctrls!(C0, C1, C2, C3, C4, C5);
impl_push_plugin_ctrls!(C0, C1, C2, C3, C4, C5, C6);
impl_push_plugin_ctrls!(C0, C1, C2, C3, C4, C5, C6, C7);
impl_push_plugin_ctrls!(C0, C1, C2, C3, C4, C5, C6, C7, C8);
impl_push_plugin_ctrls!(C0, C1, C2, C3, C4, C5, C6, C7, C8, C9);
impl_push_plugin_ctrls!(C0, C1, C2, C3, C4, C5, C6, C7, C8, C9, C10);
impl_push_plugin_ctrls!(C0, C1, C2, C3, C4, C5, C6, C7, C8, C9, C10, C11);
impl_push_plugin_ctrls!(C0, C1, C2, C3, C4, C5, C6, C7, C8, C9, C10, C11, C12);
impl_push_plugin_ctrls!(C0, C1, C2, C3, C4, C5, C6, C7, C8, C9, C10, C11, C12, C13);
impl_push_plugin_ctrls!(C0, C1, C2, C3, C4, C5, C6, C7, C8, C9, C10, C11, C12, C13, C14);
impl_push_plugin_ctrls!(C0, C1, C2, C3, C4, C5, C6, C7, C8, C9, C10, C11, C12, C13, C14, C15);

/// Registers a plugin's controller tuple into the typed builder, **with** the
/// global dependency check.
///
/// Unlike feature-module controllers (checked module-locally at
/// `register_module`, so they may inject *private* module beans), a plugin's
/// controllers may only inject beans that are in the application's provision
/// list — the plugin's own [`Provided`](crate::plugin::Plugin::Provided) tuple
/// included, since it joined `P` at `.plugin(..)`. The check therefore runs
/// here, at `build_state()`, against the **final** state HList: order-
/// independent, exactly like [`Plugin::Deps`](crate::plugin::Plugin::Deps).
///
/// `W` collects one `(extraction-marker, dependency-indices)` witness pair per
/// element; it is always inferred. Implemented for `()` and tuples of arity
/// 1..=16.
#[diagnostic::on_unimplemented(
    message = "the `{Pl}` plugin's controllers cannot be registered",
    label = "installed here",
    note = "every type in `{Pl}::Controllers` must be a `#[controller]` type whose `#[inject]` dependencies are all in the application's provision list",
    note = "`.provide(..)` / `.register::<_>()` the missing bean, or add it to `{Pl}::Provided`"
)]
pub trait PluginControllerList<Pl, T: Clone + Send + Sync + 'static, W> {
    /// Register every controller in the tuple, in tuple order.
    ///
    /// Fallible: see [`ModuleControllers::register_all`].
    fn register_all(builder: AppBuilder<T>) -> Result<AppBuilder<T>, crate::beans::BeanError>;
}

impl<Pl, T: Clone + Send + Sync + 'static> PluginControllerList<Pl, T, ()> for () {
    fn register_all(builder: AppBuilder<T>) -> Result<AppBuilder<T>, crate::beans::BeanError> {
        Ok(builder)
    }
}

macro_rules! impl_plugin_controllers {
    ($C0:ident $W0:ident $D0:ident) => {
        // `do_not_recommend`: a missing bean otherwise surfaces as the inner
        // `AllSatisfied`/`Contains` error on the controller's dependency, with
        // no hint that a *plugin* pulled that controller in. Suppressing this
        // impl from recommendation makes the compiler report the unsatisfied
        // `PluginControllerList` bound, whose message names the plugin.
        #[diagnostic::do_not_recommend]
        impl<Pl, T, $C0, $W0, $D0> PluginControllerList<Pl, T, (($W0, $D0),)> for ($C0,)
        where
            T: Clone + Send + Sync + 'static,
            $C0: Controller<T, $W0>,
            $C0::Deps: crate::type_list::AllSatisfied<T, $D0>,
        {
            fn register_all(
                builder: AppBuilder<T>,
            ) -> Result<AppBuilder<T>, crate::beans::BeanError> {
                register_deferred::<T, $C0, $W0>(builder)
            }
        }
    };
    ($C0:ident $W0:ident $D0:ident, $($Cs:ident $Ws:ident $Ds:ident),+) => {
        #[diagnostic::do_not_recommend]
        impl<Pl, T, $C0, $W0, $D0, $($Cs, $Ws, $Ds),+>
            PluginControllerList<Pl, T, (($W0, $D0), $(($Ws, $Ds)),+)> for ($C0, $($Cs),+)
        where
            T: Clone + Send + Sync + 'static,
            $C0: Controller<T, $W0>,
            $C0::Deps: crate::type_list::AllSatisfied<T, $D0>,
            $($Cs: Controller<T, $Ws>,)+
            $($Cs::Deps: crate::type_list::AllSatisfied<T, $Ds>,)+
        {
            fn register_all(
                builder: AppBuilder<T>,
            ) -> Result<AppBuilder<T>, crate::beans::BeanError> {
                let builder = register_deferred::<T, $C0, $W0>(builder)?;
                $(let builder = register_deferred::<T, $Cs, $Ws>(builder)?;)+
                Ok(builder)
            }
        }
        impl_plugin_controllers!($($Cs $Ws $Ds),+);
    };
}

impl_plugin_controllers!(
    C0 W0 D0, C1 W1 D1, C2 W2 D2, C3 W3 D3, C4 W4 D4, C5 W5 D5, C6 W6 D6,
    C7 W7 D7, C8 W8 D8, C9 W9 D9, C10 W10 D10, C11 W11 D11, C12 W12 D12,
    C13 W13 D13, C14 W14 D14, C15 W15 D15
);
