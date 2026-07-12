//! Feature modules — closed subgraphs with compile-time encapsulation.
//!
//! A [`FeatureModule`] bundles **providers (beans/producers) + controllers +
//! imports/exports** into one unit registered with a single call:
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

use crate::beans::{BeanRegistry, Registrable};
use crate::builder::AppBuilder;
use crate::controller::{Controller, EndpointDeps};
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

    /// **Tuple** of pre-state plugin types this module requires (or `()` for
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
    type RequiredPlugins;
}

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

/// The module's local resolution scope: everything its providers provide,
/// plus its imports. Provider and controller dependencies must resolve here.
pub type ModuleScope<M> = <<<M as FeatureModule>::Providers as BeanList>::Provided as TAppend<
    <M as FeatureModule>::Imports,
>>::Output;

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
    note = "`Exports` must be a subset of the providers' provided types — add a provider for `{H}` or remove it from the module's `Exports`"
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
/// in its [`RawPreStatePlugin::Provisions`](crate::plugin::RawPreStatePlugin::Provisions)
/// is present in the provision list `Self` (the app-global `P`).
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
    Plug: crate::plugin::RawPreStatePlugin,
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

impl_controller_deps_list!(
    C0, C1, C2, C3, C4, C5, C6, C7, C8, C9, C10, C11, C12, C13, C14, C15
);

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
    fn register_all(builder: AppBuilder<T>) -> AppBuilder<T>;
}

impl<T: Clone + Send + Sync + 'static> ModuleControllers<T, ()> for () {
    fn register_all(builder: AppBuilder<T>) -> AppBuilder<T> {
        builder
    }
}

macro_rules! impl_module_controllers {
    ($C0:ident $W0:ident) => {
        impl<T, $C0, $W0> ModuleControllers<T, ($W0,)> for ($C0,)
        where
            T: Clone + Send + Sync + 'static,
            $C0: Controller<T, $W0>,
        {
            fn register_all(builder: AppBuilder<T>) -> AppBuilder<T> {
                builder.register_controller_unchecked_impl::<$C0, $W0>()
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
            fn register_all(builder: AppBuilder<T>) -> AppBuilder<T> {
                builder
                    .register_controller_unchecked_impl::<$C0, $W0>()
                    $(.register_controller_unchecked_impl::<$Cs, $Ws>())+
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
    fn register_controllers(builder: AppBuilder<T>) -> AppBuilder<T>;
}

impl<T: Clone + Send + Sync + 'static> ModuleList<T, ()> for TNil {
    fn register_controllers(builder: AppBuilder<T>) -> AppBuilder<T> {
        builder
    }
}

impl<T, M, Rest, WC, WR> ModuleList<T, (WC, WR)> for TCons<M, Rest>
where
    T: Clone + Send + Sync + 'static,
    M: FeatureModule,
    M::Controllers: ModuleControllers<T, WC>,
    Rest: ModuleList<T, WR>,
{
    fn register_controllers(builder: AppBuilder<T>) -> AppBuilder<T> {
        // `Mods` grows head-first (the most recently registered module is the
        // head), so recurse into the tail first to preserve registration order.
        <M::Controllers as ModuleControllers<T, WC>>::register_all(Rest::register_controllers(
            builder,
        ))
    }
}
