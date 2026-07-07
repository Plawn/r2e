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
