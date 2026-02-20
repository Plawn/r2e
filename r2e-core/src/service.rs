use std::future::Future;
use tokio_util::sync::CancellationToken;

/// A background service that participates in DI but doesn't handle HTTP.
///
/// Implement this trait for long-running background components (queue
/// consumers, gRPC servers, metrics exporters, etc.) that need access
/// to the application state but are not HTTP handlers.
///
/// # Example
///
/// ```ignore
/// struct MetricsExporter {
///     pool: SqlitePool,
/// }
///
/// impl ServiceComponent<Services> for MetricsExporter {
///     fn from_state(state: &Services) -> Self {
///         Self { pool: state.pool.clone() }
///     }
///
///     async fn start(self, shutdown: CancellationToken) {
///         loop {
///             tokio::select! {
///                 _ = shutdown.cancelled() => break,
///                 _ = tokio::time::sleep(Duration::from_secs(60)) => {
///                     // export metrics...
///                 }
///             }
///         }
///     }
/// }
///
/// // Register in builder:
/// AppBuilder::new()
///     .build_state::<Services, _, _>().await
///     .spawn_service::<MetricsExporter>()
///     .serve("0.0.0.0:3000").await
/// ```
pub trait ServiceComponent<S>: Sized + Send + 'static {
    /// Construct from application state.
    fn from_state(state: &S) -> Self;

    /// Run until the shutdown token is cancelled.
    fn start(self, shutdown: CancellationToken) -> impl Future<Output = ()> + Send;
}
