//! Provision bundles — one struct, many `.provide()` calls.
//!
//! An [`App::Env`](crate::App::Env) that carries several long-lived resources
//! (a pool per database, a client, a lock guard, the loaded config) is
//! provisioned one line per field:
//!
//! ```ignore
//! b.override_config(env.config)      // must come *before* load_config
//!  .provide(env.doris)
//!  .provide(env.postgres)
//!  .provide(env.s3)
//!  // … six more
//! ```
//!
//! [`ProvideBundle`] collapses that to `b.provide_all(env)`. The derive emits
//! exactly the chain above — the compile-time provision list `P` grows with one
//! entry per field, in field order — so nothing about the bean graph, the state
//! HList, or the compile-time checks changes:
//!
//! ```ignore
//! #[derive(ProvideBundle)]
//! pub struct AppEnv {
//!     pub config: R2eConfig,   // → override_config(...), not a bean
//!     pub doris: DorisPool,    // → .provide(...)
//!     pub s3: S3Client,        // → .provide(...)
//! }
//!
//! b.provide_all(env).load_config::<Settings>()
//! ```
//!
//! See the derive's documentation (`r2e_macros::ProvideBundle`) for the field
//! rules: the single `R2eConfig` field, `Option<T>` fields, and the textual
//! `R2eConfig` detection.

use crate::builder::{AppBuilder, NoState};

/// A struct whose fields are provisioned in one
/// [`provide_all`](AppBuilder::provide_all) call.
///
/// Implemented by `#[derive(ProvideBundle)]`; hand-written impls are possible
/// but must keep [`OutP`](Self::OutP) in sync with the chain
/// [`provide_into`](Self::provide_into) actually performs — the two are what
/// the application state is built from.
///
/// The trait is generic over the whole builder shape (`P`, `R`, `Mods`) because
/// it consumes and returns the builder; only `P` changes (a bundle provisions
/// pre-built values, so it declares no new requirements and registers no
/// module).
pub trait ProvideBundle<P, R, Mods> {
    /// The provision list after every field has been provided — exactly what
    /// writing the `.provide()` chain by hand would produce.
    type OutP;

    /// Provide every field, in field order (an `R2eConfig` field is applied as
    /// [`override_config`](AppBuilder::override_config) first).
    fn provide_into(
        self,
        builder: AppBuilder<NoState, P, R, Mods>,
    ) -> AppBuilder<NoState, Self::OutP, R, Mods>;
}
