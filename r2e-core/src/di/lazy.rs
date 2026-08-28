use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use crate::rt::sync::OnceCell;

/// Boxed async factory for a lazy bean, held until first access.
///
/// The **closure** is `Send + Sync` (the slot lives in an `Arc<BeanContext>`
/// that is shared across threads), but the **future it returns is not**: an
/// async bean constructor's future is `!Send` by design — see
/// [`AsyncBean::build`](crate::beans::AsyncBean::build). Every resolution path
/// below therefore *creates* the future on the thread that will drive it,
/// never moving one between threads.
type LazyFactory<T> = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = T>>> + Send + Sync>;

/// Factory shape for the deprecated public [`Lazy<T>`] wrapper.
///
/// Unlike [`LazyFactory`], this one stays `Send`: [`Lazy::get`] awaits the
/// factory inline, so the future is part of whatever task calls it — including
/// request handlers, which require `Send` futures.
type SendLazyFactory<T> = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = T> + Send>> + Send + Sync>;

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
                let mut trace: Vec<&'static str> = stack[pos..].iter().map(|(_, n)| *n).collect();
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
/// context's `lazy_slots` map. On first `get::<T>()`, the factory runs on
/// the current multi-thread runtime (`block_in_place` + `block_on`) — or,
/// when that is not usable, on the control-plane runtime (sharded workers)
/// or the global fallback runtime via [`resolve_on`] — and the result is
/// cached in `OnceLock`.
///
/// **Runtime note:** this requires a multi-thread runtime, a
/// registered control-plane handle, or the `lazy-fallback-runtime` feature
/// (which covers current-thread runtimes and runtime-less threads).
pub(crate) struct LazySlot<T: Clone + Send + Sync + 'static> {
    cell: OnceLock<T>,
    factory: std::sync::Mutex<Option<LazyFactory<T>>>,
}

impl<T: Clone + Send + Sync + 'static> LazySlot<T> {
    pub(crate) fn new<F>(factory: F) -> Self
    where
        F: FnOnce() -> Pin<Box<dyn Future<Output = T>>> + Send + Sync + 'static,
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

/// Resolve `factory` on the runtime behind `handle`, blocking the current
/// thread until it completes.
///
/// The factory future is `!Send` (see [`LazyFactory`]), so it cannot be handed
/// to `handle.spawn`. Instead a dedicated OS thread is started, enters
/// `handle`'s runtime, and drives the future there with
/// [`RuntimeHandle::block_on`](crate::rt::RuntimeHandle::block_on): the
/// **closure** crosses the thread boundary (it is `Send + Sync`), the future is
/// created and polled entirely on the new thread, and any runtime resources the
/// constructor opens still bind to `handle`'s long-lived reactor — not to a
/// throwaway runtime that would be dropped underneath them.
///
/// Legal from ANY context — including from within async execution, where
/// calling `block_on` directly would panic — because the wait on this thread is
/// a plain `join()` and the `block_on` happens on a thread no runtime drives.
/// A factory panic is re-raised on this thread with its original payload.
///
/// `handle` must be a multi-thread runtime (both call sites check): a
/// `current_thread` handle would have `block_on` fight the runtime's real
/// driver.
///
/// CAUTION: while this thread waits on `join()`, whatever runtime it was
/// driving is stalled — on a `current_thread` runtime every other in-flight
/// task stops being polled until the factory completes. Lazy beans should be
/// resolved eagerly during state construction; this path exists so an
/// off-main-runtime first-touch is *correct*, not so it is cheap.
///
/// Known limitations: the circular-lazy-dependency detector (`RESOLVING`
/// thread-local) does not see across threads, so a factory that circularly
/// re-touches the bean being resolved deadlocks instead of panicking with a
/// cycle trace (same-thread detection on the main runtime is unaffected — this
/// helper is never used there). And because the factory runs off a runtime
/// worker thread, `block_in_place` inside a lazy constructor panics; do the
/// blocking work in `spawn_blocking` instead.
fn resolve_on<T>(
    handle: &crate::rt::RuntimeHandle,
    factory: LazyFactory<T>,
    runtime_desc: &str,
) -> T
where
    T: Send + 'static,
{
    let handle = handle.clone();
    let join = std::thread::Builder::new()
        .name("r2e-lazy-bean".to_owned())
        .spawn(move || handle.block_on(factory()))
        .unwrap_or_else(|err| {
            panic!(
                "failed to spawn the resolver thread for lazy bean {} on the {runtime_desc}: {err}",
                std::any::type_name::<T>()
            )
        });
    match join.join() {
        Ok(value) => value,
        // Re-raise the factory's own panic on this thread so the caller
        // sees the original payload, not a generic message.
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn resolve_lazy_factory<T>(factory: LazyFactory<T>) -> T
where
    T: Send + 'static,
{
    // Control-plane resolution (sharded mode): a sharded SO_REUSEPORT worker
    // runs a `current_thread` runtime, so a lazy bean first touched from within
    // a worker cannot use `block_in_place` (it panics on current_thread
    // runtimes). When the worker thread has a control-plane handle registered
    // (see `crate::rt::set_control_plane`), resolve the factory on the
    // control-plane (main multi-thread) runtime.
    if let Some(handle) = crate::rt::control_plane_handle() {
        tracing::debug!(
            bean = std::any::type_name::<T>(),
            "resolving lazy bean on the control-plane runtime"
        );
        return resolve_on(&handle, factory, "control-plane runtime");
    }

    match crate::rt::RuntimeHandle::try_current() {
        Some(handle) => {
            if handle.is_multi_thread() {
                crate::rt::block_in_place(|| handle.block_on(factory()))
            } else {
                // A current_thread runtime without a control plane. `block_on`
                // on the fallback runtime would panic here whenever we are
                // inside async execution ("Cannot start a runtime from within
                // a runtime"), so route through the same spawn+channel
                // mechanism as the control-plane path.
                #[cfg(feature = "lazy-fallback-runtime")]
                {
                    tracing::debug!(
                        bean = std::any::type_name::<T>(),
                        "resolving lazy bean on the lazy-fallback runtime"
                    );
                    resolve_on(
                        &fallback_runtime().handle(),
                        factory,
                        "lazy-fallback runtime",
                    )
                }
                #[cfg(not(feature = "lazy-fallback-runtime"))]
                {
                    panic!(
                        "Lazy bean resolution requires a multi-thread runtime. \
                         Enable the `lazy-fallback-runtime` feature to allow a \
                         fallback runtime."
                    )
                }
            }
        }
        None => {
            #[cfg(feature = "lazy-fallback-runtime")]
            {
                resolve_on(
                    &fallback_runtime().handle(),
                    factory,
                    "lazy-fallback runtime",
                )
            }
            #[cfg(not(feature = "lazy-fallback-runtime"))]
            {
                panic!(
                    "Lazy bean resolution requires an async runtime. \
                     Enable the `lazy-fallback-runtime` feature to allow a \
                     fallback runtime."
                )
            }
        }
    }
}

/// Test-only accessor exercising the real [`resolve_lazy_factory`] path.
///
/// Exposed as `pub` + `#[doc(hidden)]` (repo convention) so integration tests in
/// `tests/` can drive control-plane lazy resolution without `#[cfg(test)]`
/// visibility hacks. Not part of the public API.
#[doc(hidden)]
pub fn __resolve_lazy_factory_for_tests<T>(factory: LazyFactory<T>) -> T
where
    T: Send + 'static,
{
    resolve_lazy_factory(factory)
}

#[cfg(feature = "lazy-fallback-runtime")]
fn fallback_runtime() -> &'static crate::rt::Runtime {
    static RUNTIME: OnceLock<crate::rt::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        crate::rt::RuntimeBuilder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build fallback runtime for lazy beans")
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
    /// async) because the critical section is just `Option::take()` — no
    /// `.await` while holding the lock.
    factory: std::sync::Mutex<Option<SendLazyFactory<T>>>,
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
