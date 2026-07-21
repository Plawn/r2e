use std::any::TypeId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use r2e_core::beans::{Bean, BeanContext, BeanRegistry};
use r2e_core::lazy::Lazy;
use r2e_core::type_list::TNil;

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".into())
}

// ── Lazy<T> (deprecated wrapper) ────────────────────────────────────────────

#[r2e_core::test]
async fn lazy_resolves_once() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = Arc::clone(&calls);
    let lazy: Lazy<u32> = Lazy::new(move || {
        let c = Arc::clone(&c);
        Box::pin(async move {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            42
        })
    });

    assert_eq!(*lazy.get().await, 42);
    assert_eq!(*lazy.get().await, 42);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[r2e_core::test]
async fn lazy_clone_shares_cell() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&calls);
    let lazy: Lazy<u32> = Lazy::new(move || {
        let c = Arc::clone(&c);
        Box::pin(async move {
            c.fetch_add(1, Ordering::SeqCst);
            7
        })
    });

    let clone = lazy.clone();
    assert_eq!(*clone.get().await, 7);
    assert_eq!(*lazy.get().await, 7);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// ── resolve_lazy_factory paths ──────────────────────────────────────────────

#[r2e_core::test]
async fn resolve_lazy_factory_on_multi_thread_runtime() {
    // Run on a real worker thread: `block_in_place` requires one.
    let value = tokio::spawn(async {
        r2e_core::lazy::__resolve_lazy_factory_for_tests(Box::new(|| Box::pin(async { 5u32 })))
    })
    .await
    .unwrap();
    assert_eq!(value, 5);
}

#[cfg(not(feature = "lazy-fallback-runtime"))]
#[test]
fn resolve_lazy_factory_current_thread_runtime_panics() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rt.block_on(async {
            r2e_core::lazy::__resolve_lazy_factory_for_tests(Box::new(|| Box::pin(async { 1u32 })))
        })
    }));
    let msg = panic_message(result.unwrap_err());
    assert!(
        msg.contains("multi-thread runtime"),
        "unexpected panic message: {msg}"
    );
}

#[cfg(not(feature = "lazy-fallback-runtime"))]
#[test]
fn resolve_lazy_factory_without_runtime_panics() {
    let result = std::panic::catch_unwind(|| {
        r2e_core::lazy::__resolve_lazy_factory_for_tests(Box::new(|| Box::pin(async { 1u32 })))
    });
    let msg = panic_message(result.unwrap_err());
    assert!(
        msg.contains("requires a Tokio runtime"),
        "unexpected panic message: {msg}"
    );
}

#[cfg(feature = "lazy-fallback-runtime")]
#[test]
fn resolve_lazy_factory_falls_back_without_runtime() {
    // No runtime, no control plane: the `lazy-fallback-runtime` feature
    // routes resolution onto the global fallback runtime instead of panicking.
    let value = r2e_core::lazy::__resolve_lazy_factory_for_tests(Box::new(|| {
        Box::pin(async { 3u32 })
    }));
    assert_eq!(value, 3);
}

#[cfg(feature = "lazy-fallback-runtime")]
#[test]
fn resolve_lazy_factory_falls_back_on_current_thread_runtime() {
    // First-touch from WITHIN async execution on a current_thread runtime —
    // the case `Runtime::block_on` cannot serve ("Cannot start a runtime from
    // within a runtime"). The fallback routes through spawn+channel instead,
    // stalling this runtime while the factory completes on the fallback one.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (value, factory_thread) = rt.block_on(async {
        r2e_core::lazy::__resolve_lazy_factory_for_tests(Box::new(|| {
            Box::pin(async { (4u32, std::thread::current().id()) })
        }))
    });
    assert_eq!(value, 4);
    // The factory ran on the fallback runtime's workers, not on this thread.
    assert_ne!(factory_thread, std::thread::current().id());
}

/// The single worker thread of `rt` — spawned tasks land there, so the id
/// identifies where a control-plane-resolved factory actually ran.
fn single_worker_id(rt: &tokio::runtime::Runtime) -> std::thread::ThreadId {
    rt.block_on(async {
        tokio::spawn(async { std::thread::current().id() })
            .await
            .unwrap()
    })
}

#[test]
fn resolve_lazy_factory_uses_control_plane_when_registered() {
    // A thread with no runtime of its own but a registered control-plane
    // handle (the sharded-worker situation) resolves on the control plane.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let worker_id = single_worker_id(&rt);
    let handle = rt.handle().clone();
    let (value, factory_thread) = std::thread::spawn(move || {
        r2e_core::rt::set_control_plane(handle);
        r2e_core::lazy::__resolve_lazy_factory_for_tests(Box::new(|| {
            Box::pin(async { (11u32, std::thread::current().id()) })
        }))
    })
    .join()
    .unwrap();
    assert_eq!(value, 11);
    // The factory must run on the control-plane worker — not on the calling
    // thread, and not on the `lazy-fallback-runtime` global runtime (which
    // would also produce the right value if this branch regressed).
    assert_eq!(factory_thread, worker_id);
}

#[test]
fn resolve_lazy_factory_control_plane_panic_resurfaces() {
    // A factory panic on the control plane is re-raised on the calling
    // thread with the original payload.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let worker_id = single_worker_id(&rt);
    let handle = rt.handle().clone();
    // Side-channel: record where the factory ran before it panics, so the
    // test can't silently pass through the fallback-runtime path.
    let seen = Arc::new(std::sync::Mutex::new(None::<std::thread::ThreadId>));
    let seen_in_factory = Arc::clone(&seen);
    let result = std::thread::spawn(move || {
        r2e_core::rt::set_control_plane(handle);
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            r2e_core::lazy::__resolve_lazy_factory_for_tests::<()>(Box::new(move || {
                Box::pin(async move {
                    *seen_in_factory.lock().unwrap() = Some(std::thread::current().id());
                    panic!("factory boom")
                })
            }))
        }))
    })
    .join()
    .unwrap();
    let msg = panic_message(result.unwrap_err());
    assert!(msg.contains("factory boom"), "unexpected payload: {msg}");
    assert_eq!(*seen.lock().unwrap(), Some(worker_id));
}

// ── Circular lazy dependency detection ──────────────────────────────────────

#[derive(Clone)]
struct CycleLazyA;
#[derive(Clone)]
struct CycleLazyB;

impl Bean for CycleLazyA {
    type Deps = TNil;
    const LAZY: bool = true;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<CycleLazyB>(), std::any::type_name::<CycleLazyB>())]
    }
    fn build(ctx: &BeanContext) -> Self {
        let _ = ctx.get::<CycleLazyB>();
        CycleLazyA
    }
}

impl Bean for CycleLazyB {
    type Deps = TNil;
    const LAZY: bool = true;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<CycleLazyA>(), std::any::type_name::<CycleLazyA>())]
    }
    fn build(ctx: &BeanContext) -> Self {
        let _ = ctx.get::<CycleLazyA>();
        CycleLazyB
    }
}

#[r2e_core::test]
async fn circular_lazy_dependency_panics_with_cycle_trace() {
    let mut reg = BeanRegistry::new();
    reg.register::<CycleLazyA>();
    reg.register::<CycleLazyB>();
    // Lazy-to-lazy deps pass resolution — the cycle only exists at
    // first-touch time.
    let ctx = reg.resolve().await.unwrap();

    let err = tokio::spawn(async move {
        let _ = ctx.get::<CycleLazyA>();
    })
    .await
    .unwrap_err();
    assert!(err.is_panic());
    let msg = panic_message(err.into_panic());
    assert!(
        msg.contains("circular lazy bean dependency detected"),
        "unexpected panic message: {msg}"
    );
    assert!(msg.contains("CycleLazyA") && msg.contains("CycleLazyB"));
}
