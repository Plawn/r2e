use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::OnceCell;

/// A lazy bean wrapper that defers construction to first access.
///
/// Register a bean as lazy via `.with_lazy_async_bean::<T>()` on the builder.
/// Consumers declare `Lazy<T>` instead of `T` — this makes the deferred
/// construction explicit at the type level.
///
/// # Example
///
/// ```ignore
/// #[bean]
/// impl ExpensiveService {
///     async fn new(pool: SqlitePool) -> Self {
///         Self { model: load_model().await }
///     }
/// }
///
/// // Consumer declares Lazy<ExpensiveService>
/// #[bean]
/// impl MyController {
///     fn new(service: Lazy<ExpensiveService>) -> Self {
///         Self { service }
///     }
/// }
///
/// // First access triggers construction
/// let svc = self.service.get().await;
/// ```
pub struct Lazy<T: Clone + Send + Sync + 'static> {
    inner: Arc<LazyInner<T>>,
}

struct LazyInner<T: Clone + Send + Sync + 'static> {
    cell: OnceCell<T>,
    /// Holds the factory until first access. Uses `std::sync::Mutex` (not
    /// tokio) because the critical section is just `Option::take()` — no
    /// `.await` while holding the lock.
    factory: std::sync::Mutex<
        Option<Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = T> + Send>> + Send + Sync>>,
    >,
}

impl<T: Clone + Send + Sync + 'static> Lazy<T> {
    /// Create a new lazy bean with the given async factory.
    pub fn new<F>(factory: F) -> Self
    where
        F: FnOnce() -> Pin<Box<dyn Future<Output = T> + Send>> + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(LazyInner {
                cell: OnceCell::new(),
                factory: std::sync::Mutex::new(Some(Box::new(factory))),
            }),
        }
    }

    /// Get the lazily-initialized value, constructing it on first access.
    ///
    /// The factory is called at most once; subsequent calls return the
    /// cached value immediately.
    pub async fn get(&self) -> &T {
        self.inner
            .cell
            .get_or_init(|| async {
                let factory = self
                    .inner
                    .factory
                    .lock()
                    .expect("Lazy factory mutex poisoned")
                    .take()
                    .expect("Lazy factory already consumed (this is a bug in r2e)");
                factory().await
            })
            .await
    }
}

impl<T: Clone + Send + Sync + 'static> Clone for Lazy<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}
