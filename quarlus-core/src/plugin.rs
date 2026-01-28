use crate::builder::AppBuilder;

/// A composable unit of functionality that can be installed into an [`AppBuilder`].
///
/// Plugins replace the old `with_cors()`, `with_tracing()`, etc. methods with a
/// single, uniform `.with(plugin)` entry point. This makes the builder extensible
/// without requiring new methods on `AppBuilder` for every cross-cutting concern.
///
/// # Built-in plugins
///
/// See [`crate::plugins`] for the plugins shipped with `quarlus-core`:
/// [`Cors`](crate::plugins::Cors), [`Tracing`](crate::plugins::Tracing),
/// [`Health`](crate::plugins::Health), [`ErrorHandling`](crate::plugins::ErrorHandling),
/// [`DevReload`](crate::plugins::DevReload).
///
/// # Example
///
/// ```ignore
/// use quarlus_core::plugins::{Cors, Tracing, Health, ErrorHandling, DevReload};
///
/// AppBuilder::new()
///     .build_state::<Services>()
///     .with(Health)
///     .with(Cors::permissive())
///     .with(Tracing)
///     .with(ErrorHandling)
///     .with(DevReload)
/// ```
pub trait Plugin<T: Clone + Send + Sync + 'static> {
    /// Install this plugin into the given `AppBuilder`, returning the modified builder.
    fn install(self, app: AppBuilder<T>) -> AppBuilder<T>;
}
