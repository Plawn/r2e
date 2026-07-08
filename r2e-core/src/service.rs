use std::future::Future;
use tokio_util::sync::CancellationToken;

/// A background service that participates in DI but doesn't handle HTTP.
///
/// Implement this trait for long-running background components (queue
/// consumers, gRPC servers, metrics exporters, etc.) that need access
/// to application beans but are not HTTP handlers. Construction pulls beans
/// from the resolved graph by type — the same model as controller cores.
///
/// # Example
///
/// ```ignore
/// struct MetricsExporter {
///     pool: SqlitePool,
/// }
///
/// impl ServiceComponent for MetricsExporter {
///     fn from_context(ctx: &BeanContext) -> Self {
///         Self { pool: ctx.get::<SqlitePool>() }
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
///     .provide(pool)
///     .build_state().await
///     .spawn_service::<MetricsExporter>()
///     .serve("0.0.0.0:3000").await
/// ```
pub trait ServiceComponent: Sized + Send + 'static {
    /// Construct from the resolved bean graph.
    fn from_context(ctx: &crate::beans::BeanContext) -> Self;

    /// Run until the shutdown token is cancelled.
    fn start(self, shutdown: CancellationToken) -> impl Future<Output = ()> + Send;
}
