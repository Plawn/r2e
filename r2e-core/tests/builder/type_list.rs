//! Tests for the value-level HList state machinery: `HasBean` indexed access,
//! the witness-free `BeanAccess::get` façade, `BuildHList` materialization from
//! a resolved `BeanContext`, `Contains`/`AllSatisfied` over `HCons`, and the
//! `BeanState` wrapper that holds the whole list behind one `Arc`.

use r2e_core::type_list::{
    AllSatisfied, BeanAccess, BeanLookup, BeanState, BuildHList, HCons, HNil, HasBean, TCons, TNil,
};
use r2e_core::BeanRegistry;

#[derive(Clone, Debug, PartialEq)]
struct Alpha(u32);

#[derive(Clone, Debug, PartialEq)]
struct Beta(String);

#[derive(Clone, Debug, PartialEq)]
struct Gamma(bool);

fn sample_state() -> HCons<Alpha, HCons<Beta, HCons<Gamma, HNil>>> {
    HCons {
        head: Alpha(1),
        tail: HCons {
            head: Beta("two".into()),
            tail: HCons {
                head: Gamma(true),
                tail: HNil,
            },
        },
    }
}

#[test]
fn has_bean_resolves_every_slot() {
    let state = sample_state();
    let a: Alpha = state.get();
    let b: Beta = state.get();
    let c: Gamma = state.get();
    assert_eq!(a, Alpha(1));
    assert_eq!(b, Beta("two".into()));
    assert_eq!(c, Gamma(true));
}

#[test]
fn bean_access_turbofish_names_only_the_bean_type() {
    let state = sample_state();
    // No witness parameter at the call site.
    assert_eq!(state.get::<Beta>(), Beta("two".into()));
}

#[test]
fn has_bean_works_through_a_generic_fn_with_witness_param() {
    fn pull<S, I>(state: &S) -> Gamma
    where
        S: HasBean<Gamma, I>,
    {
        state.get_bean()
    }
    assert_eq!(pull(&sample_state()), Gamma(true));
}

#[test]
fn hlist_state_is_clone() {
    let state = sample_state();
    let cloned = state.clone();
    assert_eq!(cloned.get::<Alpha>(), state.get::<Alpha>());
}

#[tokio::test]
async fn build_hlist_materializes_from_context_in_list_order() {
    let mut reg = BeanRegistry::new();
    reg.provide(Alpha(7));
    reg.provide(Beta("hello".into()));
    reg.provide(Gamma(false));
    let ctx = reg.resolve().await.unwrap();

    // Shape mirrors a provision list built by three `.provide()` calls
    // (newest first): TCons<Gamma, TCons<Beta, TCons<Alpha, TNil>>>.
    type P = TCons<Gamma, TCons<Beta, TCons<Alpha, TNil>>>;
    let state: <P as BuildHList>::Output = <P as BuildHList>::build_hlist(&ctx);

    assert_eq!(state.get::<Alpha>(), Alpha(7));
    assert_eq!(state.get::<Beta>(), Beta("hello".into()));
    assert_eq!(state.get::<Gamma>(), Gamma(false));
    // Order is preserved: head is the newest provision.
    assert_eq!(state.head, Gamma(false));
}

#[tokio::test]
async fn build_hlist_empty_list() {
    let reg = BeanRegistry::new();
    let ctx = reg.resolve().await.unwrap();
    let HNil = <TNil as BuildHList>::build_hlist(&ctx);
}

#[test]
fn all_satisfied_holds_against_value_hlist_state_type() {
    // Requirement lists (TCons chains) can be checked against the materialized
    // state type with the same machinery used against the provision list.
    fn assert_satisfied<Reqs, S, W>()
    where
        Reqs: AllSatisfied<S, W>,
    {
    }
    type State = HCons<Alpha, HCons<Beta, HCons<Gamma, HNil>>>;
    assert_satisfied::<TCons<Gamma, TCons<Alpha, TNil>>, State, _>();
    assert_satisfied::<TNil, State, _>();
}

// ── `BeanState`: the HList behind one Arc (task #992) ───────────────────────

/// A bean that records every `Clone` of itself, so a test can see refcount
/// traffic an allocation counter would miss (task #992).
/// `N` only makes the types distinct: an HList slot is addressed by type.
#[derive(Debug)]
struct Counted<const N: usize>(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl<const N: usize> Clone for Counted<N> {
    fn clone(&self) -> Self {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Counted(std::sync::Arc::clone(&self.0))
    }
}

#[test]
fn bean_state_delegates_has_bean_to_the_inner_list() {
    let state = BeanState::new(sample_state());

    // Same witness-free façade, same fixed-offset access — through the Arc.
    assert_eq!(state.get::<Alpha>(), Alpha(1));
    assert_eq!(state.get::<Beta>(), Beta("two".into()));
    assert_eq!(state.get::<Gamma>(), Gamma(true));
}

#[test]
fn bean_state_delegates_has_bean_through_a_generic_witness_fn() {
    fn pull<S, I>(state: &S) -> Gamma
    where
        S: HasBean<Gamma, I>,
    {
        state.get_bean()
    }
    // The index witness still resolves: the wrapper forwards `HasBean<T, Idx>`
    // with `Idx` unchanged, so generated extractors keep their witnesses.
    assert_eq!(pull(&BeanState::new(sample_state())), Gamma(true));
}

#[test]
fn bean_state_clone_does_not_clone_any_bean() {
    // This is the whole point of the wrapper: the HTTP backend clones the
    // router state on every request, and that must not touch the beans.
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let state = BeanState::new(HCons {
        head: Counted::<0>(std::sync::Arc::clone(&counter)),
        tail: HCons {
            head: Counted::<1>(std::sync::Arc::clone(&counter)),
            tail: HNil,
        },
    });
    counter.store(0, std::sync::atomic::Ordering::Relaxed);

    for _ in 0..100 {
        let _clone = state.clone();
    }

    assert_eq!(
        counter.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "cloning the router state must be one Arc bump, not one clone per bean"
    );

    // Reading a bean out still clones that one bean, as it always did.
    let _: Counted<1> = state.get();
    assert_eq!(counter.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn bean_state_derefs_to_the_inner_list() {
    let state = BeanState::new(sample_state());
    assert_eq!(state.list().head, Alpha(1));
    assert_eq!(state.head, Alpha(1)); // via Deref
}

#[test]
fn bean_state_delegates_bean_lookup() {
    let state = BeanState::new(sample_state());
    assert_eq!(state.bean::<Beta>(), Some(Beta("two".into())));
    assert_eq!(state.bean_ref::<Gamma>(), Some(&Gamma(true)));
    // Absent types still report `None` rather than failing to compile.
    assert_eq!(state.bean::<u8>(), None);
}

#[test]
fn all_satisfied_holds_against_the_wrapped_state_type() {
    // Controller `Deps`, gRPC/MCP service registration and module checks are
    // all `AllSatisfied<StateType, _>` bounds — they must see through the
    // wrapper, which is what the delegated `Contains` impl buys.
    fn assert_satisfied<Reqs, S, W>()
    where
        Reqs: AllSatisfied<S, W>,
    {
    }
    type State = BeanState<HCons<Alpha, HCons<Beta, HCons<Gamma, HNil>>>>;
    assert_satisfied::<TCons<Gamma, TCons<Alpha, TNil>>, State, _>();
    assert_satisfied::<TNil, State, _>();
}

#[tokio::test]
async fn build_bean_state_wraps_the_materialized_list() {
    let mut reg = BeanRegistry::new();
    reg.provide(Alpha(7));
    reg.provide(Beta("hello".into()));
    reg.provide(Gamma(false));
    let ctx = reg.resolve().await.unwrap();

    type P = TCons<Gamma, TCons<Beta, TCons<Alpha, TNil>>>;
    let state: BeanState<<P as BuildHList>::Output> = <P as BuildHList>::build_bean_state(&ctx);

    assert_eq!(state.get::<Alpha>(), Alpha(7));
    assert_eq!(state.head, Gamma(false));
}
