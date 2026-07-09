//! Tests for the value-level HList state machinery: `HasBean` indexed access,
//! the witness-free `BeanAccess::get` façade, `BuildHList` materialization from
//! a resolved `BeanContext`, and `Contains`/`AllSatisfied` over `HCons`.

use r2e_core::type_list::{AllSatisfied, BeanAccess, BuildHList, HCons, HNil, HasBean, TCons, TNil};
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
