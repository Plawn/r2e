use std::future::Future;
use std::pin::Pin;

/// A startup hook that receives a reference to the application state.
pub type StartupHook<T> =
    Box<dyn FnOnce(T) -> Pin<Box<dyn Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send>> + Send>;

/// A shutdown hook that runs when the server stops.
pub type ShutdownHook<T> =
    Box<dyn FnOnce(T) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// A drain hook, awaited when shutdown is triggered — after the signal (or
/// [`StopHandle::stop`]) but **before** the server stops accepting
/// connections. See [`AppBuilder::on_drain`](crate::AppBuilder::on_drain).
pub type DrainHook<T> = ShutdownHook<T>;

/// Handle to stop a running server programmatically.
///
/// Triggers the exact same graceful-shutdown sequence as an OS signal
/// (Ctrl-C/SIGTERM): drain hooks are awaited, the listener stops accepting,
/// in-flight requests finish, and shutdown hooks run. The `run()`/`serve()`
/// future resolves once the whole sequence completes.
///
/// Obtain one from [`PreparedApp::stop_handle`](crate::PreparedApp::stop_handle),
/// or create it first with [`StopHandle::new`] and `provide()` it as a bean
/// (e.g. for an admin stop endpoint) — a `StopHandle` bean is wired into the
/// lifecycle automatically at `prepare()`.
///
/// # Example
///
/// ```ignore
/// let prepared = app.prepare("127.0.0.1:0");
/// let stop = prepared.stop_handle();
/// let server = tokio::spawn(prepared.run());
/// // ... exercise the app ...
/// stop.stop();
/// server.await.unwrap().unwrap(); // resolves after graceful drain
/// ```
#[derive(Clone, Debug, Default)]
pub struct StopHandle {
    token: tokio_util::sync::CancellationToken,
}

impl StopHandle {
    /// Create a fresh, untriggered stop handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request a graceful shutdown. Idempotent; returns immediately —
    /// await the server's `run()` future to observe drain completion.
    pub fn stop(&self) {
        self.token.cancel();
    }

    /// Whether [`stop`](Self::stop) has been called.
    pub fn is_stopped(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Resolves once [`stop`](Self::stop) has been called.
    pub async fn stopped(&self) {
        self.token.cancelled().await;
    }
}

/// Boxed future returned by fallible lifecycle methods
/// ([`LifecycleController::on_start`],
/// [`PostConstruct::post_construct`](crate::beans::PostConstruct::post_construct)).
pub type LifecycleFuture<'a> = Pin<
    Box<dyn Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send + 'a>,
>;

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
///     fn on_stop(_state: &Services) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
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
    fn on_start(_state: &T) -> LifecycleFuture<'_> {
        Box::pin(async { Ok(()) })
    }

    /// Called once after the server shuts down.
    ///
    /// Default implementation does nothing.
    fn on_stop(_state: &T) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}
