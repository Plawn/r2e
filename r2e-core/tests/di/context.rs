//! `BeanContext`: clone, overlay, snapshot semantics, `Debug`.

use std::any::{type_name, TypeId};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use r2e_core::beans::{Bean, BeanContext, BeanRegistry, PreDestroy};
use r2e_core::type_list::TNil;

use crate::lazy_bean::{LazyCounter, Probe};

// ── BeanContext: clone, overlay, Debug ──────────────────────────────────────

#[r2e_core::test]
async fn bean_context_clone_shares_lazy_slots() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = BeanRegistry::new();
    reg.provide(Probe {
        calls: calls.clone(),
    });
    reg.register::<LazyCounter>();
    let ctx = reg.resolve().await.unwrap();
    let snapshot = ctx.clone();

    // Resolve through the clone; the original must see the cached value.
    let (ctx, snapshot) = tokio::spawn(async move {
        assert_eq!(snapshot.get::<LazyCounter>().n, 5);
        assert_eq!(ctx.get::<LazyCounter>().n, 5);
        (ctx, snapshot)
    })
    .await
    .unwrap();
    drop((ctx, snapshot));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn bean_context_empty_has_no_beans_and_debug_shows_counts() {
    let ctx = BeanContext::empty();
    assert!(ctx.try_get::<Probe>().is_none());
    let dbg = format!("{ctx:?}");
    assert!(dbg.contains("base_count: 0"), "debug output: {dbg}");
    assert!(dbg.contains("lazy_count: 0"), "debug output: {dbg}");
}

#[derive(Clone)]
struct CtxHolder {
    snap: BeanContext,
}

impl Bean for CtxHolder {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn build(ctx: &BeanContext) -> Self {
        // Holding a context clone (as a lazy-factory snapshot would) forces
        // later insertions onto the overlay instead of the shared base.
        CtxHolder { snap: ctx.clone() }
    }
}

#[derive(Clone)]
struct AfterHolder;

impl Bean for AfterHolder {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<CtxHolder>(), type_name::<CtxHolder>())]
    }
    fn build(ctx: &BeanContext) -> Self {
        let _ = ctx.get::<CtxHolder>();
        AfterHolder
    }
}

#[r2e_core::test]
async fn bean_context_snapshot_does_not_see_later_beans() {
    let mut reg = BeanRegistry::new();
    reg.register::<CtxHolder>();
    reg.register::<AfterHolder>();
    let ctx = reg.resolve().await.unwrap();

    // The final context sees both beans (AfterHolder landed on the overlay).
    let holder: CtxHolder = ctx.get();
    let _: AfterHolder = ctx.get();

    // The snapshot taken during CtxHolder's construction predates both
    // insertions — neither bean is visible through it.
    assert!(holder.snap.try_get::<AfterHolder>().is_none());
    assert!(holder.snap.try_get::<CtxHolder>().is_none());
}

#[derive(Clone)]
struct DisposeFlag {
    flag: Arc<AtomicBool>,
}

impl PreDestroy for DisposeFlag {
    fn pre_destroy(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.flag.store(true, Ordering::SeqCst);
        })
    }
}

#[r2e_core::test]
async fn bean_context_clone_does_not_carry_disposers() {
    let mut reg = BeanRegistry::new();
    reg.provide(DisposeFlag {
        flag: Arc::new(AtomicBool::new(false)),
    });
    reg.register_pre_destroy::<DisposeFlag>();
    let mut ctx = reg.resolve().await.unwrap();

    let mut snapshot = ctx.clone();
    assert!(snapshot.take_disposers().is_empty());
    assert_eq!(ctx.take_disposers().len(), 1);
}
