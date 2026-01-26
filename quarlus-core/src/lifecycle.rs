use std::future::Future;
use std::pin::Pin;

/// A startup hook that receives a reference to the application state.
pub type StartupHook<T> =
    Box<dyn FnOnce(T) -> Pin<Box<dyn Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send>> + Send>;

/// A shutdown hook that runs when the server stops.
pub type ShutdownHook =
    Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// Trait for controllers that define lifecycle methods.
///
/// Controllers implementing this trait can provide startup and shutdown
/// hooks that are automatically registered when the controller is added
/// to the `AppBuilder`.
///
/// # Example
///
/// ```ignore
/// impl LifecycleController<Services> for MyController {
///     fn on_start(state: &Services) -> Pin<Box<dyn Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send + '_>> {
///         Box::pin(async move {
///             tracing::info!("MyController starting up");
///             Ok(())
///         })
///     }
///
///     fn on_stop() -> Pin<Box<dyn Future<Output = ()> + Send>> {
///         Box::pin(async {
///             tracing::info!("MyController shutting down");
///         })
///     }
/// }
/// ```
pub trait LifecycleController<T: Clone + Send + Sync + 'static> {
    /// Called once before the server starts listening.
    ///
    /// Default implementation does nothing.
    fn on_start(
        _state: &T,
    ) -> Pin<Box<dyn Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    /// Called once after the server shuts down.
    ///
    /// Default implementation does nothing.
    fn on_stop() -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async {})
    }
}
