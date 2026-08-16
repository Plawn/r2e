#[allow(unused_imports)] // referenced by intra-doc links
use super::PreStatePlugin;
use crate::builder::AppBuilder;

// ── Post-state Plugin trait ────────────────────────────────────────────────

/// A composable unit of functionality that can be installed into an [`AppBuilder`].
///
/// Plugins are installed after `build_state()` is called. They can:
/// - Add layers to the router
/// - Register routes
/// - Register startup/shutdown hooks
///
/// For plugins that need to provide beans (like Scheduler), use [`PreStatePlugin`]
/// instead.
///
/// # Example
///
/// ```ignore
/// use r2e_core::Plugin;
///
/// pub struct Health;
///
/// impl Plugin for Health {
///     fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
///         app.register_routes(Router::new().route("/health", get(|| async { "OK" })))
///     }
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `Plugin`, the post-state plugin API used by `.with()`",
    label = "`.with()` needs a post-state `Plugin`",
    note = "if `{Self}` is a pre-state plugin (it provides beans), install it with `.plugin({Self})` BEFORE `build_state()` instead of `.with({Self})`"
)]
pub trait Plugin: Send + 'static {
    /// Install this plugin into the given `AppBuilder`, returning the modified builder.
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T>;

    /// Whether this plugin should be installed last in the layer stack.
    ///
    /// Some plugins need to be the outermost layer (installed last) to work
    /// correctly. When `should_be_last()` returns `true`, the builder will
    /// warn if other plugins are added after this one.
    fn should_be_last() -> bool
    where
        Self: Sized,
    {
        false
    }

    /// The name of this plugin (for diagnostics).
    fn name() -> &'static str
    where
        Self: Sized,
    {
        std::any::type_name::<Self>()
    }
}
