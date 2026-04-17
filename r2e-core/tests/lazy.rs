use std::sync::Arc;

use r2e_core::lazy::Lazy;

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
