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
