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
