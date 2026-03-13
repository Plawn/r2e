//! Bundle system for R2E.
//!
//! Bundles group bean registrations and controller wiring into reusable units
//! that can be exported from separate crates and composed in the application.
//!
//! # Two traits
//!
//! - [`Bundle`]: Pre-state phase — registers beans into the dependency graph.
//! - [`BundleRoutes<T>`]: Post-state phase — registers controllers into the router.
//!
//! A typical bundle implements both traits:
//!
//! ```ignore
//! use r2e_core::prelude::*;
//! use r2e_core::bundle::{Bundle, BundleRoutes};
//! use r2e_core::type_list::{TCons, TNil, TAppend};
//!
//! pub struct UserBundle;
//!
//! impl Bundle for UserBundle {
//!     type Provisions = TCons<UserService, TCons<UserRepository, TNil>>;
//!     type Required = TCons<Pool, TNil>;
//!
//!     fn beans<P, R>(app: AppBuilder<NoState, P, R>)
//!         -> AppBuilder<NoState,
//!             <P as TAppend<Self::Provisions>>::Output,
//!             <R as TAppend<Self::Required>>::Output>
//!     where
//!         P: TAppend<Self::Provisions>,
//!         R: TAppend<Self::Required>,
//!     {
//!         app.with_bean::<UserRepository>()
//!            .with_bean::<UserService>()
//!     }
//! }
//!
//! impl<T> BundleRoutes<T> for UserBundle
//! where
//!     T: Clone + Send + Sync + 'static,
//!     UserController: Controller<T>,
//! {
//!     fn routes(app: AppBuilder<T>) -> AppBuilder<T> {
//!         app.register_controller::<UserController>()
//!     }
//! }
//! ```
//!
//! # Usage
//!
//! ```ignore
//! AppBuilder::new()
//!     .bundle_beans::<UserBundle>()
//!     .bundle_beans::<AccountBundle>()
//!     .build_state::<Services, _, _>().await
//!     .bundle_routes::<UserBundle>()
//!     .bundle_routes::<AccountBundle>()
//!     .serve("0.0.0.0:3000").await;
//! ```

use crate::builder::{AppBuilder, NoState};
use crate::type_list::TAppend;

/// A bundle that registers beans in the pre-state phase.
///
/// `Provisions` declares the bean types this bundle provides to the dependency
/// graph. `Required` declares the bean types this bundle needs but does not
/// provide — they must come from another bundle, plugin, or `.provide()` call.
///
/// Both are type-level lists (`TCons<A, TCons<B, TNil>>`) checked at compile
/// time by [`AppBuilder::build_state()`].
pub trait Bundle: Send + 'static {
    /// Type-level list of bean types this bundle provides.
    type Provisions;

    /// Type-level list of bean types this bundle requires from external sources.
    type Required;

    /// Register beans into the pre-state builder.
    fn beans<P, R>(
        app: AppBuilder<NoState, P, R>,
    ) -> AppBuilder<
        NoState,
        <P as TAppend<Self::Provisions>>::Output,
        <R as TAppend<Self::Required>>::Output,
    >
    where
        P: TAppend<Self::Provisions>,
        R: TAppend<Self::Required>;
}

/// A bundle that registers controllers in the post-state phase.
///
/// Generic over `T` (the application state type). The implementation adds
/// `where` clauses for each controller it registers:
///
/// ```ignore
/// impl<T> BundleRoutes<T> for MyBundle
/// where
///     T: Clone + Send + Sync + 'static,
///     MyController: Controller<T>,
/// {
///     fn routes(app: AppBuilder<T>) -> AppBuilder<T> {
///         app.register_controller::<MyController>()
///     }
/// }
/// ```
///
/// These bounds are checked at compile time when calling
/// [`AppBuilder::bundle_routes()`].
pub trait BundleRoutes<T: Clone + Send + Sync + 'static> {
    /// Register controllers, subscribers, and post-state plugins into the builder.
    fn routes(app: AppBuilder<T>) -> AppBuilder<T>;
}
