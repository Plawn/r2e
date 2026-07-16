//! Decorator construction: guards and interceptors as graph-resolved beans.
//!
//! A *decorator* is a guard or an interceptor applied to a route via
//! `#[guard(...)]` / `#[pre_guard(...)]` / `#[intercept(...)]`. Decorators
//! are built **once, at controller registration**, from the resolved
//! [`BeanContext`](crate::beans::BeanContext) — never per request — and the
//! beans they read are declared at the type level so a missing bean is a
//! compile error at `register_controller()`, exactly like a missing
//! `#[inject]` bean.
//!
//! The attribute expression's **leading type path** names the spec:
//! `#[guard(RateLimit::per_user(5, 60))]` has spec type `RateLimit`, and the
//! expression must evaluate to it. `#[routes]` emits, per site:
//!
//! ```ignore
//! <RateLimit as DecoratorSpec>::build(RateLimit::per_user(5, 60), ctx)
//! ```
//!
//! and folds `<RateLimit as DecoratorSpec>::Deps` into the controller's
//! `Deps` list.
//!
//! Two ways to implement the contract:
//!
//! - **Self-contained decorators** (no bean deps, the expression already is
//!   the finished guard/interceptor — `RolesGuard`, `Logged`, `Timed`, unit
//!   guards): opt in with one line, `impl SelfBuilt for MyGuard {}`.
//! - **Bean-reading decorators**: the expression evaluates to a pure config
//!   value whose `DecoratorSpec` impl names the product and its deps, and
//!   pulls them in `build`:
//!
//! ```ignore
//! impl DecoratorSpec for RateLimit {
//!     type Product = RateLimitGuard;
//!     type Deps = TCons<RateLimitRegistry, TNil>;
//!     fn build(self, ctx: &BeanContext) -> RateLimitGuard {
//!         RateLimitGuard { registry: ctx.get(), config: self }
//!     }
//! }
//! ```

use crate::beans::BeanContext;
use crate::type_list::TNil;

/// Hidden per-core storage for prebuilt decorator sets that must be reachable
/// from `&self`.
///
/// Scheduled-method interceptor chains run inside the method body so that
/// **direct in-code calls** are intercepted too (not just scheduler ticks),
/// and the body's only handle is the core. `#[controller]` adds a `DecoSlot`
/// field to every physical core; the generated `scheduled_tasks_boxed` fills
/// it at registration with the sets built from the resolved graph
/// ([`DecoratorSpec::build`]), so build-once semantics are preserved.
///
/// A core that was never registered (hand-built via
/// [`ContextConstruct::from_context`](crate::ContextConstruct::from_context)
/// in a test) has an empty slot: its scheduled methods run undecorated when
/// called directly. Cloning a core yields a fresh empty slot — the clone is
/// not registered.
#[doc(hidden)]
#[derive(Default)]
pub struct DecoSlot(std::sync::OnceLock<Box<dyn std::any::Any + Send + Sync>>);

impl DecoSlot {
    pub fn new() -> Self {
        Self(std::sync::OnceLock::new())
    }

    /// Fill the slot. Later calls are ignored — the first registration wins,
    /// and a core is only ever registered once.
    pub fn fill<T: Send + Sync + 'static>(&self, sets: T) {
        let _ = self.0.set(Box::new(sets));
    }

    /// The prebuilt sets, if the core went through registration.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.0.get().and_then(|b| b.downcast_ref::<T>())
    }
}

/// A cloned core is a new, unregistered core: fresh empty slot. Keeps
/// `#[derive(Clone)]` on user controller structs working.
impl Clone for DecoSlot {
    fn clone(&self) -> Self {
        Self::new()
    }
}

/// Keeps `#[derive(Debug)]` on user controller structs working.
impl std::fmt::Debug for DecoSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DecoSlot")
    }
}

/// Hidden per-**bean** storage for prebuilt decorator sets, the bean-level
/// counterpart of [`DecoSlot`].
///
/// A `#[bean]` whose `#[scheduled]` / `#[consumer]` methods carry
/// `#[intercept(...)]` runs its interceptor chain inside the method body (so
/// **direct in-code calls** are intercepted too, not just scheduler ticks /
/// event delivery), and the body's only handle is `&self`. `#[bean]` on the
/// **struct** injects a hidden `SharedDecoSlot` field (reached through
/// [`HasDecoSlot`]); the fill runs once, from the resolved graph, during
/// `build_state()`.
///
/// # Contrast with [`DecoSlot`]
///
/// [`DecoSlot`] clones **empty** — controller cores live behind a single
/// `Arc`, so the clone-resets-slot behaviour is harmless. Beans are cloned by
/// **value** everywhere: `ctx.get::<T>()` hands out clones, and dependents
/// capture clones during graph resolution — *before* the slot is filled.
/// `SharedDecoSlot` therefore **shares** its storage across clones (the inner
/// `Arc` is cloned, not the contents), so a fill through any one handle is
/// visible to every clone already handed out. Filling `DecoSlot` on a fresh
/// clone would be lost; filling `SharedDecoSlot` on any clone is seen by all.
///
/// A bean whose type was pinned (`override_bean`) skips registration, so its
/// slot is never filled and its methods run undecorated — same semantics as a
/// skipped `#[post_construct]` / `#[scheduled]` source.
#[doc(hidden)]
#[derive(Default)]
pub struct SharedDecoSlot(
    std::sync::Arc<std::sync::OnceLock<Box<dyn std::any::Any + Send + Sync>>>,
);

impl SharedDecoSlot {
    pub fn new() -> Self {
        Self(std::sync::Arc::new(std::sync::OnceLock::new()))
    }

    /// Fill the shared slot. Later calls are ignored — the first fill wins,
    /// and a bean type is only ever filled once per graph (deduped by
    /// `TypeId` in [`BeanRegistry::register_deco_fill`]).
    ///
    /// [`BeanRegistry::register_deco_fill`]: crate::beans::BeanRegistry::register_deco_fill
    pub fn fill<T: Send + Sync + 'static>(&self, sets: T) {
        let _ = self.0.set(Box::new(sets));
    }

    /// The prebuilt sets, if the bean went through registration.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.0.get().and_then(|b| b.downcast_ref::<T>())
    }
}

/// Sharing is the whole point (see the type docs): cloning shares the inner
/// `Arc` so every clone observes the same fill.
impl Clone for SharedDecoSlot {
    fn clone(&self) -> Self {
        Self(std::sync::Arc::clone(&self.0))
    }
}

/// Keeps `#[derive(Debug)]` on user bean structs working.
impl std::fmt::Debug for SharedDecoSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedDecoSlot")
    }
}

/// Access to a bean's hidden [`SharedDecoSlot`].
///
/// Implemented by `#[bean]` on a **struct** (which injects the hidden
/// `__r2e_decos` field). The generated interceptor wrappers on a `#[bean]`
/// impl reach the slot **only** through this trait — never the field directly
/// — so that forgetting `#[bean]` on the struct produces the diagnostic below
/// instead of a raw "no field `__r2e_decos`" error.
#[diagnostic::on_unimplemented(
    message = "`{Self}` has no decorator slot",
    label = "missing the hidden `#[bean]` decorator slot",
    note = "add `#[bean]` on `struct {Self}` — `#[intercept]` on a bean `#[scheduled]`/`#[consumer]` method needs the hidden decorator slot injected by the struct attribute"
)]
pub trait HasDecoSlot {
    fn __r2e_deco_slot(&self) -> &SharedDecoSlot;
}

/// Fill hook contract for a bean's decorator slot.
///
/// Generated by `#[bean]` on an impl whose `#[scheduled]`/`#[consumer]`
/// methods carry `#[intercept(...)]`. `__r2e_fill_decos` builds the per-bean
/// decorator container from the resolved graph
/// ([`DecoratorSpec::build`]) and fills the shared slot. Registered as a
/// `build_state()` hook via
/// [`BeanRegistry::register_deco_fill`](crate::beans::BeanRegistry::register_deco_fill).
pub trait BeanDecoFill: Send + Sync + 'static {
    /// Build every intercepted method's decorator set from `ctx` and fill the
    /// bean's [`SharedDecoSlot`]. Called once per bean type, at registration.
    ///
    /// `Clone` is **not** a supertrait: a bean's fill hook is registered via
    /// [`BeanRegistry::register_deco_fill`](crate::beans::BeanRegistry::register_deco_fill)
    /// (which pulls the bean by value from the graph, so it bounds `Clone`
    /// there), while a controller core — which is not `Clone` — impls this trait
    /// too and is filled from its own `Arc` at registration.
    fn __r2e_fill_decos(&self, ctx: &BeanContext);
}

/// Opt-in decoration for **hand-built** bean instances that never went through
/// normal registration (so the `build_state()` fill hook never ran for them).
///
/// The blanket impl is available on every bean whose `#[bean]` impl generated a
/// [`BeanDecoFill`]. Call [`decorate`](Decorate::decorate) with the resolved
/// [`BeanContext`] to build the interceptor chains from the graph and fill the
/// instance's slot:
///
/// ```ignore
/// let svc = CleanupService::new(stub_pool);   // hand-built, unregistered
/// svc.decorate(app.bean_context());           // build + fill from the graph
/// svc.purge().await;                           // now intercepted
/// ```
///
/// Because the slot is a [`SharedDecoSlot`] (Arc-shared across clones), a clone
/// taken **after** `decorate` shares the fill. `decorate` is **idempotent** —
/// the underlying `OnceLock` is first-write-wins, so a second call is a no-op.
///
/// This is the explicit escape hatch for tests; normal registration fills the
/// slot automatically (and a plain `override_bean` pin leaves it undecorated —
/// see `override_bean_decorated` to re-enable decoration on a pinned instance).
pub trait Decorate: BeanDecoFill {
    /// Build this bean's interceptor chains from the resolved graph and fill
    /// its decorator slot. Idempotent (first fill wins); clones taken after
    /// this call share the fill.
    fn decorate(&self, ctx: &BeanContext) {
        self.__r2e_fill_decos(ctx);
    }
}

impl<T: BeanDecoFill> Decorate for T {}

/// Construction contract for guards and interceptors.
///
/// Implemented by the type named by the attribute expression's leading type
/// path. `build` runs once per site at controller registration, with the
/// resolved bean graph; `Deps` is folded into the controller's `Deps` and
/// checked against the application state at `register_controller()`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a guard/interceptor spec",
    label = "not usable in #[guard(...)] / #[pre_guard(...)] / #[intercept(...)]",
    note = "for a self-contained guard or interceptor, add `impl SelfBuilt for {Self} {{}}`; for a config type that reads beans, implement `DecoratorSpec` (Product + Deps + build)"
)]
pub trait DecoratorSpec: Sized {
    /// The guard or interceptor this spec builds.
    type Product: Send + Sync + 'static;

    /// Type-level list ([`TCons`](crate::type_list::TCons) /
    /// [`TNil`](crate::type_list::TNil)) of the bean types `build` pulls
    /// from the context.
    type Deps;

    /// Build the decorator from the resolved graph. Called once per
    /// attribute site, at registration time.
    fn build(self, ctx: &BeanContext) -> Self::Product;
}

/// Opt-in marker for decorators that are their own spec: the attribute
/// expression evaluates directly to the finished guard/interceptor, which
/// reads no beans (`Product = Self`, `Deps = TNil`).
///
/// ```ignore
/// pub struct RequireApiKey(pub &'static str);
/// impl SelfBuilt for RequireApiKey {}
/// ```
///
/// The blanket [`DecoratorSpec`] impl below coexists with manual config-type
/// impls in downstream crates: a local type without a `SelfBuilt` impl can
/// never gain one elsewhere (orphan rules), so there is no overlap. A type
/// must not be both — the compiler rejects the ambiguity.
pub trait SelfBuilt {}

impl<D: SelfBuilt + Send + Sync + 'static> DecoratorSpec for D {
    type Product = D;
    type Deps = TNil;

    fn build(self, _ctx: &BeanContext) -> D {
        self
    }
}

/// Build one decorator site: `spec` is the attribute expression's value,
/// `Named` the spec type extracted from its leading path. `#[routes]` emits
/// every site through this function.
///
/// For hand-written specs the two coincide (`S = Named`). With
/// `#[derive(DecoratorBean)]` they split — the expression
/// (`DbAuditLog::spec(..)`) evaluates to a hidden config type while the
/// leading path names the product — and the equality bounds keep the split
/// honest: whatever the expression builds must have exactly the `Product`
/// and `Deps` declared by the named type, so the controller's dep fold
/// (which uses `Named::Deps`) always covers what `build` pulls.
pub fn build_decorator<S, Named>(spec: S, ctx: &BeanContext) -> Named::Product
where
    Named: DecoratorSpec,
    S: DecoratorSpec<Product = Named::Product, Deps = Named::Deps>,
{
    spec.build(ctx)
}
