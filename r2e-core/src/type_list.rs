//! Compile-time type-level list for tracking provided bean types.
//!
//! This module implements a type-level linked list that enables the compiler to
//! verify at compile time that all required types have been provided to the
//! [`AppBuilder`](crate::AppBuilder) before calling `build_state()`.
//!
//! # How It Works
//!
//! The type-level list is built using two types:
//! - [`TNil`]: Represents an empty list
//! - [`TCons<H, T>`]: A "cons cell" where `H` is the head type and `T` is the tail
//!
//! When you call `.provide(value)` or `.register::<T>()` on the builder, the
//! provision type parameter grows:
//!
//! ```text
//! AppBuilder<NoState, TNil>                    // Initial: empty list
//!     .provide(pool)                           // → AppBuilder<NoState, TCons<Pool, TNil>>
//!     .register::<UserService>()              // → AppBuilder<NoState, TCons<UserService, TCons<Pool, TNil>>>
//! ```
//!
//! # Compile-Time Verification
//!
//! When `build_state()` is called, the requirement list `R` (every dependency
//! declared by registered beans and, at `register_controller()`, each
//! controller's `Deps`) is checked against the provision list `P` via
//! [`AllSatisfied<P, W>`]. `AllSatisfied` walks the requirement list and
//! resolves one [`Contains`] bound per element.
//!
//! If a required type is missing, the compiler produces a clear error message
//! thanks to `#[diagnostic::on_unimplemented]`.
//!
//! `build_state()` then materializes `P` into a value-level [`HCons`] chain
//! (via [`BuildHList`]) — the application state, with no hand-written struct.
//!
//! # Example
//!
//! ```ignore
//! // This compiles:
//! AppBuilder::new()
//!     .provide(pool)
//!     .register::<UserService>()   // UserService depends on Pool — satisfied
//!     .build_state()
//!     .await
//!
//! // This fails at compile time with a helpful error:
//! AppBuilder::new()
//!     .register::<UserService>()   // ✗ Pool never provided
//!     .build_state()
//!     .await
//! // Error: type `Pool` was not provided to the AppBuilder
//! //        missing `.provide(value)` or `.register::<Pool>()`
//! ```
//!
//! # Index Witnesses
//!
//! The `Idx` parameter in [`Contains<H, Idx>`] is an "index witness" that tells
//! the compiler where in the list the type was found:
//! - [`Here`]: The type is at the head of the list
//! - [`There<I>`]: The type is somewhere in the tail (recursively)
//!
//! This mechanism avoids overlapping trait implementations and allows the compiler
//! to resolve each `Contains` bound independently.

use std::marker::PhantomData;

/// Empty type-level list.
///
/// Represents a provision list with no types. This is the initial state of
/// `AppBuilder::new()`.
pub struct TNil;

/// Cons cell: `H` is a provided type, `T` is the rest of the list.
///
/// Each call to `.provide::<T>()` or `.register::<T>()` wraps the current
/// list in a new `TCons`:
///
/// ```text
/// TCons<UserService, TCons<Pool, TNil>>
///   │                  │
///   │                  └─ Pool was provided first
///   └─ UserService was provided second
/// ```
///
/// Uses `PhantomData<fn() -> (H, T)>` to avoid imposing `Send`/`Sync`
/// constraints on phantom type parameters.
pub struct TCons<H, T>(PhantomData<fn() -> (H, T)>);

/// Index witness: the element was found at the head of the list.
///
/// Used in `Contains<H, Here>` to indicate that type `H` is the first
/// element of the type-level list.
pub struct Here;

/// Index witness: the element was found somewhere in the tail.
///
/// Used in `Contains<H, There<I>>` to indicate that type `H` is not at
/// the head, but somewhere deeper in the list at index `I`.
pub struct There<T>(PhantomData<fn() -> T>);

/// Compile-time witness that type `H` is present in the type-level list `Self`,
/// located at position `Idx`.
///
/// The `Idx` parameter is an index witness (`Here` or `There<...>`) that guides
/// the compiler's trait resolution and avoids overlapping impls. It is always
/// inferred automatically — users never need to specify it.
///
/// # Trait Implementations
///
/// ```text
/// // Base case: H is at the head
/// impl<H, T> Contains<H, Here> for TCons<H, T> {}
///
/// // Recursive case: H is in the tail
/// impl<H, X, T, I> Contains<H, There<I>> for TCons<X, T>
/// where T: Contains<H, I> {}
/// ```
#[diagnostic::on_unimplemented(
    message = "type `{H}` was not provided to the AppBuilder",
    label = "missing `.provide::<{H}>()` or `.register::<{H}>()`",
    note = "every dependency must be provided or registered as a bean before calling `build_state()`"
)]
pub trait Contains<H, Idx> {}

// Base case: H is the head of the list.
impl<H, T> Contains<H, Here> for TCons<H, T> {}

// Recursive case: H is somewhere in the tail.
impl<H, X, T, I> Contains<H, There<I>> for TCons<X, T> where T: Contains<H, I> {}

// ── Value-level HList (the application state) ────────────────────────────

/// Empty value-level list. The state of an app with no provisions.
///
/// Value-level counterpart of [`TNil`].
#[derive(Clone, Copy, Debug, Default)]
pub struct HNil;

/// Value-level cons cell: `head` is a resolved bean instance, `tail` is the
/// rest of the state.
///
/// Value-level counterpart of [`TCons`]. The application state produced by
/// [`AppBuilder::build_state`](crate::AppBuilder::build_state) is an `HCons`
/// chain whose shape mirrors the builder's provision list `P`, assembled via
/// [`BuildHList`]. Beans are accessed by type through [`HasBean`] /
/// [`BeanAccess::get`], which monomorphize to a fixed-offset field access.
#[derive(Clone, Copy, Debug)]
pub struct HCons<H, T> {
    /// The resolved bean instance at this slot.
    pub head: H,
    /// The rest of the state.
    pub tail: T,
}

/// Compile-time indexed access to a bean of type `T` stored in a value-level
/// HList state.
///
/// `Idx` is an index witness ([`Here`] / [`There`]) that guides trait
/// resolution, exactly like [`Contains`] — it is always inferred, never
/// written by users. Prefer calling [`BeanAccess::get`], which hides the
/// witness entirely: `state.get::<T>()`.
///
/// Resolution monomorphizes to a direct field access (`.tail.tail.head`) —
/// struct-speed, no `TypeId` lookup, hash, or downcast.
#[diagnostic::on_unimplemented(
    message = "bean `{T}` is not present in the application state",
    label = "`{T}` was never provided or registered on the AppBuilder",
    note = "add `.provide(value)` or `.register::<{T}>()` before `build_state()` so the bean is part of the provision list"
)]
pub trait HasBean<T, Idx> {
    /// Clone the bean of type `T` out of the state.
    fn get_bean(&self) -> T;
}

// Base case: T is the head of the list.
impl<H: Clone, Tail> HasBean<H, Here> for HCons<H, Tail> {
    #[inline(always)]
    fn get_bean(&self) -> H {
        self.head.clone()
    }
}

// Recursive case: T is somewhere in the tail.
impl<H, Tail, T, I> HasBean<T, There<I>> for HCons<H, Tail>
where
    Tail: HasBean<T, I>,
{
    #[inline(always)]
    fn get_bean(&self) -> T {
        self.tail.get_bean()
    }
}

/// Witness-free façade over [`HasBean`]: `state.get::<T>()`.
///
/// The index witness lives on the trait (`Idx`) while the bean type lives on
/// the method (`T`), so call sites name only the bean type — the compiler
/// infers `Idx` from the `Self: HasBean<T, Idx>` bound:
///
/// ```ignore
/// let service = state.get::<UserService>();
/// ```
pub trait BeanAccess<Idx> {
    /// Clone the bean of type `T` out of the state.
    fn get<T>(&self) -> T
    where
        Self: HasBean<T, Idx>;
}

impl<S, Idx> BeanAccess<Idx> for S {
    #[inline(always)]
    fn get<T>(&self) -> T
    where
        Self: HasBean<T, Idx>,
    {
        self.get_bean()
    }
}

// `Contains` also holds for the value-level HList, so requirement lists
// (`TCons` chains) can be checked against the materialized state type with the
// same `AllSatisfied` machinery used against the provision list `P`.
impl<H, T> Contains<H, Here> for HCons<H, T> {}
impl<H, X, T, I> Contains<H, There<I>> for HCons<X, T> where T: Contains<H, I> {}

// ── The router state: the HList behind one Arc ───────────────────────────

/// The application state installed on the router: the resolved bean
/// [`HCons`] chain `L` held behind a single [`Arc`](std::sync::Arc).
///
/// # Why the wrapper exists
///
/// The HTTP backend clones the router state on **every** request, whether or
/// not the handler declares `State<S>` — the router hands each handler its
/// own `state.clone()`. Installing the bare
/// `HCons` chain therefore cost the sum of every bean's `Clone` — O(N) in the
/// size of the bean graph. Beans are `Arc`-shaped by convention, so that was
/// usually N refcount bumps rather than N deep copies, but nothing *enforced*
/// it and N grows with the app.
///
/// `BeanState` makes that cost O(1): one refcount bump regardless of the
/// number of beans, and no bean's own `Clone` runs on the request path at all
/// (task #992, `docs/claude/hot-path-clone-audit.md`).
///
/// # It is still a fixed-offset access
///
/// Every access trait is forwarded to the inner list, so `state.get::<T>()`
/// still monomorphizes to `(*arc).tail.tail.head.clone()` — one pointer
/// dereference, then the same constant field offset as before. No `TypeId`
/// lookup, no hashing, no downcast:
///
/// - [`HasBean<T, Idx>`] — delegated, index witnesses preserved.
/// - [`Contains<H, Idx>`] — delegated, so `AllSatisfied` checks (controller
///   `Deps`, gRPC/MCP service registration) work against the wrapper.
/// - [`BeanLookup`] — delegated, for the witness-free `state.bean::<T>()`.
/// - [`Deref`](std::ops::Deref)`<Target = L>` — for the rare code that wants
///   the list itself.
///
/// [`BeanAccess::get`] comes along for free: it is a blanket impl over any
/// `Self: HasBean<T, Idx>`.
///
/// Built once by
/// [`AppBuilder::build_state`](crate::AppBuilder::build_state); tests that
/// hand-assemble a state wrap their list with [`BeanState::new`].
pub struct BeanState<L>(std::sync::Arc<L>);

impl<L> BeanState<L> {
    /// Wrap a materialized HList as the router state.
    #[inline]
    pub fn new(list: L) -> Self {
        BeanState(std::sync::Arc::new(list))
    }

    /// Borrow the underlying HList.
    #[inline(always)]
    pub fn list(&self) -> &L {
        &self.0
    }
}

impl<L> Clone for BeanState<L> {
    /// One refcount bump — this is the whole point of the wrapper, and it is
    /// what the HTTP backend runs per request.
    #[inline(always)]
    fn clone(&self) -> Self {
        BeanState(std::sync::Arc::clone(&self.0))
    }
}

impl<L> std::ops::Deref for BeanState<L> {
    type Target = L;

    #[inline(always)]
    fn deref(&self) -> &L {
        &self.0
    }
}

impl<L: std::fmt::Debug> std::fmt::Debug for BeanState<L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("BeanState").field(&*self.0).finish()
    }
}

impl<L, T, Idx> HasBean<T, Idx> for BeanState<L>
where
    L: HasBean<T, Idx>,
{
    #[inline(always)]
    fn get_bean(&self) -> T {
        self.0.get_bean()
    }
}

impl<L, H, Idx> Contains<H, Idx> for BeanState<L> where L: Contains<H, Idx> {}

/// Dynamic (`TypeId`-based) bean access over a value-level HList state.
///
/// The witness-free complement of [`HasBean`], for generic code that **cannot
/// carry an index witness**: trait impls where the witness would be an
/// unconstrained impl parameter (E0207) and the concrete implementor type is
/// not nameable by generated code — `ManagedResource` providers like
/// `HasPool`. (Guards and interceptors no longer read the state: they are
/// built once from the `BeanContext` via `DecoratorSpec` and hold their beans
/// as fields.)
///
/// Resolution monomorphizes to a chain of constant `TypeId` comparisons (no
/// hashing, no heap), so a lookup costs at most one integer compare per state
/// slot. Prefer [`HasBean`] / [`BeanAccess::get`] wherever a witness can be
/// threaded — it compiles to a direct field access and turns a missing bean
/// into a compile error, whereas `BeanLookup` reports absence at runtime via
/// `None`.
pub trait BeanLookup {
    /// Borrow the bean with the given `TypeId`, if present.
    fn lookup_bean(&self, tid: std::any::TypeId) -> Option<&(dyn std::any::Any + Send + Sync)>;

    /// Borrow the bean of type `T`, if present.
    fn bean_ref<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.lookup_bean(std::any::TypeId::of::<T>())?
            .downcast_ref()
    }

    /// Clone the bean of type `T` out of the state, if present.
    fn bean<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        self.bean_ref::<T>().cloned()
    }
}

impl BeanLookup for HNil {
    #[inline(always)]
    fn lookup_bean(&self, _tid: std::any::TypeId) -> Option<&(dyn std::any::Any + Send + Sync)> {
        None
    }
}

impl<H: Send + Sync + 'static, T: BeanLookup> BeanLookup for HCons<H, T> {
    #[inline(always)]
    fn lookup_bean(&self, tid: std::any::TypeId) -> Option<&(dyn std::any::Any + Send + Sync)> {
        if std::any::TypeId::of::<H>() == tid {
            Some(&self.head)
        } else {
            self.tail.lookup_bean(tid)
        }
    }
}

impl<L: BeanLookup> BeanLookup for BeanState<L> {
    #[inline(always)]
    fn lookup_bean(&self, tid: std::any::TypeId) -> Option<&(dyn std::any::Any + Send + Sync)> {
        self.0.lookup_bean(tid)
    }
}

// `Arc<S>` delegates — generated gRPC wrappers hand interceptors an
// `Arc<BeanContext>` as their state, so the same `BeanLookup`-bounded
// interceptors work on both HTTP (HList state) and gRPC.
impl<S: BeanLookup> BeanLookup for std::sync::Arc<S> {
    #[inline(always)]
    fn lookup_bean(&self, tid: std::any::TypeId) -> Option<&(dyn std::any::Any + Send + Sync)> {
        (**self).lookup_bean(tid)
    }

    fn bean<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        (**self).bean()
    }
}

/// Materialize a type-level provision list into a value-level HList state by
/// pulling each slot from the resolved [`BeanContext`](crate::beans::BeanContext).
///
/// Implemented for `TNil` / `TCons` chains. `build_state()` calls this **once
/// at startup** — one `ctx.get::<T>()` (a `TypeId` lookup + clone) per slot —
/// after which all state access is monomorphized field access via [`HasBean`].
pub trait BuildHList {
    /// The value-level HList with the same shape as this type-level list.
    type Output: Clone + Send + Sync + 'static;

    /// Pull every slot from the context, preserving list order.
    ///
    /// # Panics
    ///
    /// Panics if a slot's type is absent from the context. This should never
    /// happen: every type in the provision list `P` was registered or provided
    /// on the builder, and graph resolution fails earlier (with a proper
    /// error) if a bean could not be constructed.
    fn build_hlist(ctx: &crate::beans::BeanContext) -> Self::Output;

    /// [`build_hlist`](Self::build_hlist), wrapped in the [`BeanState`] the
    /// router is actually given — so the per-request state clone is one
    /// refcount bump instead of one per bean.
    fn build_bean_state(ctx: &crate::beans::BeanContext) -> BeanState<Self::Output> {
        BeanState::new(Self::build_hlist(ctx))
    }
}

impl BuildHList for TNil {
    type Output = HNil;

    fn build_hlist(_ctx: &crate::beans::BeanContext) -> HNil {
        HNil
    }
}

impl<H, T> BuildHList for TCons<H, T>
where
    H: Clone + Send + Sync + 'static,
    T: BuildHList,
{
    type Output = HCons<H, T::Output>;

    fn build_hlist(ctx: &crate::beans::BeanContext) -> Self::Output {
        HCons {
            head: ctx.get::<H>(),
            tail: T::build_hlist(ctx),
        }
    }
}

// ── Type-level list concatenation ────────────────────────────────────────

/// Concatenate two type-level lists.
///
/// `TNil ++ Other = Other`, `TCons<H, T> ++ Other = TCons<H, T ++ Other>`.
///
/// Used internally by `AppBuilder` to accumulate bean dependency requirements
/// from multiple bean registrations.
pub trait TAppend<Other> {
    /// The resulting concatenated list.
    type Output;
}

impl<Other> TAppend<Other> for TNil {
    type Output = Other;
}

impl<H, T, Other> TAppend<Other> for TCons<H, T>
where
    T: TAppend<Other>,
{
    type Output = TCons<H, T::Output>;
}

// ── Compile-time requirement verification ────────────────────────────────

/// Compile-time verification that every type in `Self` (the requirements list)
/// is present in the provision list `P`.
///
/// This trait is checked on [`AppBuilder::build_state()`] to ensure that all
/// bean dependencies are satisfied by the current provisions. If a bean
/// declares a dependency that was never `.provide()`-d or registered via
/// `.register()`, the compiler will emit an error at the call site.
///
/// `Indices` is an opaque witness tuple inferred by the compiler.
#[diagnostic::on_unimplemented(
    message = "one or more bean dependencies are missing from the AppBuilder",
    note = "a registered bean has a dependency that was not provided — add `.provide(value)` or `.register::<Type>()` for the missing type"
)]
pub trait AllSatisfied<P, Indices> {}

// Base case: an empty requirements list is always satisfied.
impl<P> AllSatisfied<P, ()> for TNil {}

// Recursive case: the head must be in P, and the tail must also be satisfied.
impl<H, T, P, IH, IT> AllSatisfied<P, (IH, IT)> for TCons<H, T>
where
    P: Contains<H, IH>,
    T: AllSatisfied<P, IT>,
{
}

// ── Plugin dependency resolution ─────────────────────────────────────────

/// Maps a concrete tuple type to a type-level list and resolves values from
/// the materialized bean graph.
///
/// This trait bridges compile-time plugin dependency declarations (`type Deps`)
/// with the type-level provision tracking (`TCons`/`TNil`) and runtime value
/// resolution from [`BeanContext`](crate::beans::BeanContext).
///
/// # Arity Implementations
///
/// Implementations are provided for tuples of arity 0 through 8:
///
/// | Tuple | Type-level list |
/// |---|---|
/// | `()` | `TNil` |
/// | `(A,)` | `TCons<A, TNil>` |
/// | `(A, B)` | `TCons<A, TCons<B, TNil>>` |
/// | ... | ... |
///
/// # Example
///
/// ```ignore
/// impl Plugin for MyPlugin {
///     type Provided = (MyThing,);
///     type Deps = (DbPool, CancelToken);
///     type Config = ();
///     type Controllers = ();
///
///     async fn build(
///         self,
///         (pool, token): (DbPool, CancelToken),
///         _config: Option<()>,
///         _ctx: &mut PluginBuildContext,
///     ) -> Result<(MyThing,), PluginBuildError> {
///         Ok((MyThing::new(pool, token),))
///     }
/// }
/// ```
pub trait PluginDeps: Send {
    /// The type-level list representation of these dependencies.
    type AsList;

    /// The runtime `(TypeId, type name)` pairs of these dependencies.
    ///
    /// Used as the plugin build node's edges in the bean graph's topological
    /// sort — the plugin's [`build`](crate::Plugin::build) runs only
    /// after every dependency is constructed. Generated from the tuple shape,
    /// so it can never drift from [`resolve_from_context`](Self::resolve_from_context).
    fn dependencies() -> Vec<(std::any::TypeId, &'static str)>;

    /// Resolve all dependency values from the (partially or fully)
    /// materialized [`BeanContext`](crate::beans::BeanContext).
    ///
    /// Used for [`Plugin::Deps`](crate::Plugin::Deps): the
    /// plugin's build node is topologically ordered after its dependencies,
    /// so every one of them — `.provide()`-d, `.register()`-ed
    /// (factory-built), or produced by another plugin — is available when
    /// this runs.
    ///
    /// # Panics
    ///
    /// Panics if a required bean is absent from the context. This should never
    /// happen: `Deps` is appended to the builder's requirement list and
    /// verified against the final provision list at `build_state()`.
    fn resolve_from_context(ctx: &crate::beans::BeanContext) -> Self;
}

// Arity 0
impl PluginDeps for () {
    type AsList = TNil;

    fn dependencies() -> Vec<(std::any::TypeId, &'static str)> {
        Vec::new()
    }

    fn resolve_from_context(_ctx: &crate::beans::BeanContext) -> Self {}
}

macro_rules! impl_plugin_deps {
    ($($T:ident),+) => {
        impl<$($T),+> PluginDeps for ($($T,)+)
        where
            $($T: Clone + Send + Sync + 'static),+
        {
            type AsList = impl_plugin_deps!(@list $($T),+);

            fn dependencies() -> Vec<(std::any::TypeId, &'static str)> {
                vec![$(
                    (std::any::TypeId::of::<$T>(), std::any::type_name::<$T>()),
                )+]
            }

            fn resolve_from_context(ctx: &crate::beans::BeanContext) -> Self {
                ($(
                    ctx.try_get::<$T>().unwrap_or_else(|| {
                        panic!(
                            "PluginDeps: bean `{}` not found in the resolved bean context (this is a bug — \
                             plugin `Deps` is appended to the builder's requirement list and should have been \
                             verified against the final provision list at `build_state()`)",
                            std::any::type_name::<$T>()
                        )
                    }),
                )+)
            }
        }
    };
    (@list $head:ident) => { TCons<$head, TNil> };
    (@list $head:ident, $($rest:ident),+) => { TCons<$head, impl_plugin_deps!(@list $($rest),+)> };
}

impl_plugin_deps!(A);
impl_plugin_deps!(A, B);
impl_plugin_deps!(A, B, C);
impl_plugin_deps!(A, B, C, D);
impl_plugin_deps!(A, B, C, D, E);
impl_plugin_deps!(A, B, C, D, E, F);
impl_plugin_deps!(A, B, C, D, E, F, G);
impl_plugin_deps!(A, B, C, D, E, F, G, H);

// ── Plugin provision mapping ─────────────────────────────────────────────

/// Maps a concrete tuple of provided bean types to a type-level list and
/// exposes its elements for per-type projection into the bean graph.
///
/// This is the mirror image of [`PluginDeps`]: where `PluginDeps` *reads*
/// dependency values out of the graph, `PluginProvisions` describes the beans
/// a [`Plugin`](crate::Plugin) produces. The plugin's
/// [`build`](crate::Plugin::build) runs as one graph node yielding the
/// whole tuple; each element is then projected out as its own bean via
/// [`element_ids`](Self::element_ids) + [`clone_element`](Self::clone_element).
/// It also bridges the plugin's [`Provided`](crate::Plugin::Provided)
/// tuple to the type-level provision list (`TCons`/`TNil`) tracked on the
/// builder.
///
/// # Arity Implementations
///
/// Implementations are provided for tuples of arity 0 through 8:
///
/// | Tuple | Type-level list |
/// |---|---|
/// | `()` | `TNil` |
/// | `(A,)` | `TCons<A, TNil>` |
/// | `(A, B)` | `TCons<A, TCons<B, TNil>>` |
/// | ... | ... |
///
/// A plugin that provides a **single** bean writes `type Provided = (MyThing,)`
/// and returns `(handle,)`. A plugin that provides **nothing** (only deferred
/// actions) writes `type Provided = ()`.
///
/// There is deliberately **no** scalar (non-tuple) impl — it would collide with
/// the tuple blanket impls. Single-provision plugins use the one-tuple `(T,)`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid plugin `Provided` type",
    label = "the `Provided` type must be a tuple of provided beans",
    note = "write `type Provided = (MyBean,)` for a single bean, `(A, B)` for several, or `()` for none — a bare `type Provided = MyBean` is not supported"
)]
pub trait PluginProvisions: Clone + Send + Sync + 'static {
    /// The type-level list representation of these provided beans.
    type AsList;

    /// The runtime `(TypeId, type name)` pairs of the tuple's elements, in
    /// tuple order.
    ///
    /// Each pair becomes one **projection node** in the bean graph: a bean
    /// registered under the element's `TypeId` whose factory clones the
    /// element out of the plugin's build output. Also used by the blanket
    /// [`PluginInstall`](crate::plugin::PluginInstall) impl to detect
    /// when every provided type is pinned by a test override (the whole
    /// plugin build is then skipped).
    fn element_ids() -> Vec<(std::any::TypeId, &'static str)>;

    /// Clone the element at `idx` (tuple order, matching
    /// [`element_ids`](Self::element_ids)) into a type-erased box holding the
    /// element's concrete type.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is out of range — callers iterate `element_ids()`, so
    /// this never happens.
    fn clone_element(&self, idx: usize) -> Box<dyn std::any::Any + Send + Sync>;
}

// Arity 0 — a plugin that provides no beans (only effects).
impl PluginProvisions for () {
    type AsList = TNil;

    fn element_ids() -> Vec<(std::any::TypeId, &'static str)> {
        Vec::new()
    }

    fn clone_element(&self, idx: usize) -> Box<dyn std::any::Any + Send + Sync> {
        panic!("PluginProvisions::clone_element({idx}) on an empty `Provided` tuple")
    }
}

macro_rules! impl_plugin_provisions {
    ($(($T:ident, $idx:tt)),+) => {
        impl<$($T),+> PluginProvisions for ($($T,)+)
        where
            $($T: Clone + Send + Sync + 'static),+
        {
            type AsList = impl_plugin_provisions!(@list $($T),+);

            fn element_ids() -> Vec<(std::any::TypeId, &'static str)> {
                vec![$(
                    (std::any::TypeId::of::<$T>(), std::any::type_name::<$T>()),
                )+]
            }

            fn clone_element(&self, idx: usize) -> Box<dyn std::any::Any + Send + Sync> {
                match idx {
                    $( $idx => Box::new(self.$idx.clone()), )+
                    _ => panic!(
                        "PluginProvisions::clone_element({idx}) out of range for `{}`",
                        std::any::type_name::<Self>()
                    ),
                }
            }
        }
    };
    (@list $head:ident) => { TCons<$head, TNil> };
    (@list $head:ident, $($rest:ident),+) => { TCons<$head, impl_plugin_provisions!(@list $($rest),+)> };
}

impl_plugin_provisions!((A, 0));
impl_plugin_provisions!((A, 0), (B, 1));
impl_plugin_provisions!((A, 0), (B, 1), (C, 2));
impl_plugin_provisions!((A, 0), (B, 1), (C, 2), (D, 3));
impl_plugin_provisions!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4));
impl_plugin_provisions!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5));
impl_plugin_provisions!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6));
impl_plugin_provisions!(
    (A, 0),
    (B, 1),
    (C, 2),
    (D, 3),
    (E, 4),
    (F, 5),
    (G, 6),
    (H, 7)
);

/// Registers a tuple of controllers into an [`AppBuilder`](crate::AppBuilder) in
/// one call.
///
/// This backs
/// [`RegisterControllers::register_controllers`](crate::builder::RegisterControllers::register_controllers),
/// which folds every tuple element through the single-controller registration
/// path, preserving tuple order. Implemented for tuples of arity 1..=16; each
/// element must implement [`Controller<T, Wi>`](crate::controller::Controller)
/// with its dependency list satisfied by the state, so a non-controller in the
/// tuple — or a controller with a missing bean — is a clear compile error.
///
/// `W` collects one `(Wi, Di)` witness pair per element (extraction markers +
/// dependency indices); it is always inferred.
///
/// ```ignore
/// app.register_controllers::<(UserController, AccountController, DataController)>()
/// ```
pub trait ControllerTuple<T: Clone + Send + Sync + 'static, W> {
    /// Fold every controller in the tuple through
    /// `register_controller`, in tuple order.
    fn register_all(builder: crate::builder::AppBuilder<T>) -> crate::builder::AppBuilder<T>;
}

macro_rules! impl_controller_tuple {
    ($(($C:ident, $W:ident, $D:ident)),+) => {
        impl<T, $($C, $W, $D),+> ControllerTuple<T, ($(($W, $D),)+)> for ($($C,)+)
        where
            T: Clone + Send + Sync + 'static,
            $(
                $C: crate::controller::Controller<T, $W>,
                <$C as crate::controller::Controller<T, $W>>::Deps: AllSatisfied<T, $D>,
            )+
        {
            fn register_all(
                builder: crate::builder::AppBuilder<T>,
            ) -> crate::builder::AppBuilder<T> {
                builder
                    $(.register_controller_impl::<$C, $W, $D>())+
            }
        }
    };
}

impl_controller_tuple!((C0, W0, D0));
impl_controller_tuple!((C0, W0, D0), (C1, W1, D1));
impl_controller_tuple!((C0, W0, D0), (C1, W1, D1), (C2, W2, D2));
impl_controller_tuple!((C0, W0, D0), (C1, W1, D1), (C2, W2, D2), (C3, W3, D3));
impl_controller_tuple!(
    (C0, W0, D0),
    (C1, W1, D1),
    (C2, W2, D2),
    (C3, W3, D3),
    (C4, W4, D4)
);
impl_controller_tuple!(
    (C0, W0, D0),
    (C1, W1, D1),
    (C2, W2, D2),
    (C3, W3, D3),
    (C4, W4, D4),
    (C5, W5, D5)
);
impl_controller_tuple!(
    (C0, W0, D0),
    (C1, W1, D1),
    (C2, W2, D2),
    (C3, W3, D3),
    (C4, W4, D4),
    (C5, W5, D5),
    (C6, W6, D6)
);
impl_controller_tuple!(
    (C0, W0, D0),
    (C1, W1, D1),
    (C2, W2, D2),
    (C3, W3, D3),
    (C4, W4, D4),
    (C5, W5, D5),
    (C6, W6, D6),
    (C7, W7, D7)
);
impl_controller_tuple!(
    (C0, W0, D0),
    (C1, W1, D1),
    (C2, W2, D2),
    (C3, W3, D3),
    (C4, W4, D4),
    (C5, W5, D5),
    (C6, W6, D6),
    (C7, W7, D7),
    (C8, W8, D8)
);
impl_controller_tuple!(
    (C0, W0, D0),
    (C1, W1, D1),
    (C2, W2, D2),
    (C3, W3, D3),
    (C4, W4, D4),
    (C5, W5, D5),
    (C6, W6, D6),
    (C7, W7, D7),
    (C8, W8, D8),
    (C9, W9, D9)
);
impl_controller_tuple!(
    (C0, W0, D0),
    (C1, W1, D1),
    (C2, W2, D2),
    (C3, W3, D3),
    (C4, W4, D4),
    (C5, W5, D5),
    (C6, W6, D6),
    (C7, W7, D7),
    (C8, W8, D8),
    (C9, W9, D9),
    (C10, W10, D10)
);
impl_controller_tuple!(
    (C0, W0, D0),
    (C1, W1, D1),
    (C2, W2, D2),
    (C3, W3, D3),
    (C4, W4, D4),
    (C5, W5, D5),
    (C6, W6, D6),
    (C7, W7, D7),
    (C8, W8, D8),
    (C9, W9, D9),
    (C10, W10, D10),
    (C11, W11, D11)
);
impl_controller_tuple!(
    (C0, W0, D0),
    (C1, W1, D1),
    (C2, W2, D2),
    (C3, W3, D3),
    (C4, W4, D4),
    (C5, W5, D5),
    (C6, W6, D6),
    (C7, W7, D7),
    (C8, W8, D8),
    (C9, W9, D9),
    (C10, W10, D10),
    (C11, W11, D11),
    (C12, W12, D12)
);
impl_controller_tuple!(
    (C0, W0, D0),
    (C1, W1, D1),
    (C2, W2, D2),
    (C3, W3, D3),
    (C4, W4, D4),
    (C5, W5, D5),
    (C6, W6, D6),
    (C7, W7, D7),
    (C8, W8, D8),
    (C9, W9, D9),
    (C10, W10, D10),
    (C11, W11, D11),
    (C12, W12, D12),
    (C13, W13, D13)
);
impl_controller_tuple!(
    (C0, W0, D0),
    (C1, W1, D1),
    (C2, W2, D2),
    (C3, W3, D3),
    (C4, W4, D4),
    (C5, W5, D5),
    (C6, W6, D6),
    (C7, W7, D7),
    (C8, W8, D8),
    (C9, W9, D9),
    (C10, W10, D10),
    (C11, W11, D11),
    (C12, W12, D12),
    (C13, W13, D13),
    (C14, W14, D14)
);
impl_controller_tuple!(
    (C0, W0, D0),
    (C1, W1, D1),
    (C2, W2, D2),
    (C3, W3, D3),
    (C4, W4, D4),
    (C5, W5, D5),
    (C6, W6, D6),
    (C7, W7, D7),
    (C8, W8, D8),
    (C9, W9, D9),
    (C10, W10, D10),
    (C11, W11, D11),
    (C12, W12, D12),
    (C13, W13, D13),
    (C14, W14, D14),
    (C15, W15, D15)
);
