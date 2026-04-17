use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use tokio::sync::OnceCell;

thread_local! {
    /// Stack of lazy-bean `(TypeId, type_name)` pairs currently being resolved
    /// on this thread. Used to detect circular lazy dependencies and turn the
    /// otherwise-cryptic `OnceLock::get_or_init` re-entry abort into a clear
    /// panic with the cycle trace.
    static RESOLVING: RefCell<Vec<(TypeId, &'static str)>> = const { RefCell::new(Vec::new()) };
}

/// Guard that records entry into a lazy-bean factory on the thread-local
/// resolution stack and pops on drop. Panics on re-entry (circular dep).
struct ResolutionGuard(TypeId);

impl ResolutionGuard {
    fn enter(tid: TypeId, name: &'static str) -> Self {
        RESOLVING.with(|stack| {
            let mut stack = stack.borrow_mut();
            if let Some(pos) = stack.iter().position(|(t, _)| *t == tid) {
                let mut trace: Vec<&'static str> =
                    stack[pos..].iter().map(|(_, n)| *n).collect();
                trace.push(name);
                panic!(
                    "circular lazy bean dependency detected: {}",
                    trace.join(" -> ")
                );
            }
            stack.push((tid, name));
        });
        Self(tid)
    }
}

impl Drop for ResolutionGuard {
    fn drop(&mut self) {
        RESOLVING.with(|stack| {
            let mut stack = stack.borrow_mut();
            if stack.last().map(|(t, _)| *t) == Some(self.0) {
                stack.pop();
            }
        });
    }
}

// ── LazySlot (internal) ─────────────────────────────────────────────────────

/// Type-erased lazy bean slot stored in `BeanContext::lazy_slots`.
///
/// This trait lets `get::<T>()` resolve a lazy bean without requiring
/// `T: Send + Sync` in its own signature — those bounds are captured
/// when the `LazySlot<T>` is created.
pub(crate) trait LazyResolve: Send + Sync {
    /// Resolve the lazy bean and return a reference to it.
    /// First call runs the factory; subsequent calls return the cached value.
    fn resolve(&self) -> &dyn Any;
}

/// Internal lazy bean slot used by [`BeanContext`](crate::beans::BeanContext)
/// for transparent lazy resolution.
///
/// Unlike [`Lazy<T>`], this is **not** exposed to users. When a bean is
/// marked `#[bean(lazy)]`, the registry stores a `LazySlot<T>` in the
/// context's `lazy_slots` map. On first `get::<T>()`, the factory runs
/// via `block_in_place` + `block_on` and the result is cached in `OnceLock`.
///
/// **Runtime note:** this requires a Tokio multi-thread runtime. If the
/// `lazy-fallback-runtime` feature is enabled, resolution will fall back
/// to a global runtime when none is available (or when running on a
/// current-thread runtime).
pub(crate) struct LazySlot<T: Clone + Send + Sync + 'static> {
    cell: OnceLock<T>,
    factory: std::sync::Mutex<
        Option<Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = T> + Send>> + Send + Sync>>,
    >,
}

impl<T: Clone + Send + Sync + 'static> LazySlot<T> {
    pub(crate) fn new<F>(factory: F) -> Self
    where
        F: FnOnce() -> Pin<Box<dyn Future<Output = T> + Send>> + Send + Sync + 'static,
    {
        Self {
            cell: OnceLock::new(),
            factory: std::sync::Mutex::new(Some(Box::new(factory))),
        }
    }

    fn get_or_init(&self) -> &T {
        // Fast path: already initialized — skip the resolution-stack bookkeeping.
        if let Some(v) = self.cell.get() {
            return v;
        }
        let _guard = ResolutionGuard::enter(TypeId::of::<T>(), std::any::type_name::<T>());
        self.cell.get_or_init(|| {
            let factory = self
                .factory
                .lock()
                .expect("LazySlot factory mutex poisoned")
                .take()
                .expect("LazySlot factory already consumed (possible circular lazy dependency)");
            resolve_lazy_factory(factory)
        })
    }
}

impl<T: Clone + Send + Sync + 'static> LazyResolve for LazySlot<T> {
    fn resolve(&self) -> &dyn Any {
        self.get_or_init()
    }
}

fn resolve_lazy_factory<T>(
    factory: Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = T> + Send>> + Send + Sync>,
) -> T
where
    T: Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread {
                tokio::task::block_in_place(|| handle.block_on(factory()))
            } else {
                #[cfg(feature = "lazy-fallback-runtime")]
                {
                    fallback_runtime().block_on(factory())
                }
                #[cfg(not(feature = "lazy-fallback-runtime"))]
                {
                    panic!(
                        "Lazy bean resolution requires a Tokio multi-thread runtime. \
                         Enable the `lazy-fallback-runtime` feature to allow a \
                         fallback runtime."
                    )
                }
            }
        }
        Err(_) => {
            #[cfg(feature = "lazy-fallback-runtime")]
            {
                fallback_runtime().block_on(factory())
            }
            #[cfg(not(feature = "lazy-fallback-runtime"))]
            {
                panic!(
                    "Lazy bean resolution requires a Tokio runtime. \
                     Enable the `lazy-fallback-runtime` feature to allow a \
                     fallback runtime."
                )
            }
        }
    }
}

#[cfg(feature = "lazy-fallback-runtime")]
fn fallback_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build fallback Tokio runtime for lazy beans")
    })
}

// ── Lazy<T> (public, deprecated path) ───────────────────────────────────────

/// A lazy bean wrapper that defers construction to first access.
///
/// **Deprecated pattern.** Prefer `#[bean(lazy)]` which is fully transparent —
/// consumers use `T` directly and the bean is constructed on first injection.
///
/// This type is kept for backward compatibility with code that constructed
/// `Lazy<T>` directly. The builder helpers were removed in favor of
/// `#[bean(lazy)]` which is fully transparent.
///
/// # Example (deprecated pattern)
///
/// ```ignore
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
