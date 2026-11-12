//! Service DI: what the bean graph costs to build, and what a bean costs to
//! read once it is built.

use std::cell::Cell;
use std::hint::black_box;
use std::sync::Arc;

use r2e_core::http::StatusCode;
use r2e_core::prelude::*;
use r2e_core::type_list::BeanAccess;
use r2e_core::AppBuilder;

use crate::allocations;
use crate::fixtures::allocations_per_request;

/// Allocations `build_state()` may make for each additional bean with one
/// dependency (registration, graph node, resolution, the boxed instance).
/// Checked on the marginal cost between two graph sizes, so the builder's
/// fixed cost does not count — and checked again at twice the size, where
/// anything worse than linear would show.
const ALLOCATIONS_PER_BEAN: u64 = 5;

/// Clones of a provided bean `build_state()` may make on top of one per
/// dependent: the copy that lands in the state.
const CLONES_INTO_STATE: u64 = 1;

/// Clones of an injected bean registering a controller may make: the copy
/// its core keeps. Requests then share that core behind an `Arc`.
const CLONES_PER_CONTROLLER: u64 = 1;

// ── A chain of beans, each depending on the previous one ───────────────────

#[derive(Clone)]
struct Root(Arc<u32>);

macro_rules! chain {
    ($previous:ident => $name:ident $(=> $rest:ident)*) => {
        #[derive(Clone)]
        struct $name(Arc<u32>);

        #[bean]
        impl $name {
            fn new(previous: $previous) -> Self {
                Self(previous.0)
            }
        }

        chain!($name $(=> $rest)*);
    };
    ($last:ident) => {};
}

chain!(
    Root
    => C01 => C02 => C03 => C04 => C05 => C06 => C07 => C08
    => C09 => C10 => C11 => C12 => C13 => C14 => C15 => C16
    => C17 => C18 => C19 => C20 => C21 => C22 => C23 => C24
    => C25 => C26 => C27 => C28 => C29 => C30 => C31 => C32
    => C33 => C34 => C35 => C36 => C37 => C38 => C39 => C40
    => C41 => C42 => C43 => C44 => C45 => C46 => C47 => C48
    => C49 => C50 => C51 => C52 => C53 => C54 => C55 => C56
    => C57 => C58 => C59 => C60 => C61 => C62 => C63 => C64
);

/// Build the chain up to the listed beans; evaluates to
/// `(allocations made by the build, the built app)`.
macro_rules! build_chain {
    ($($bean:ident)*) => {{
        let before = allocations();
        let app = AppBuilder::new()
            .provide(Root(Arc::new(1)))
            $(.register::<$bean>())*
            .build_state()
            .await;
        (allocations() - before, app)
    }};
}

macro_rules! chain_16 {
    ($($more:ident)*) => {
        build_chain!(
            C01 C02 C03 C04 C05 C06 C07 C08 C09 C10 C11 C12 C13 C14 C15 C16
            $($more)*
        )
    };
}

macro_rules! chain_32 {
    ($($more:ident)*) => {
        chain_16!(
            C17 C18 C19 C20 C21 C22 C23 C24 C25 C26 C27 C28 C29 C30 C31 C32
            $($more)*
        )
    };
}

macro_rules! chain_64 {
    () => {
        chain_32!(
            C33 C34 C35 C36 C37 C38 C39 C40 C41 C42 C43 C44 C45 C46 C47 C48
            C49 C50 C51 C52 C53 C54 C55 C56 C57 C58 C59 C60 C61 C62 C63 C64
        )
    };
}

#[r2e_core::test(flavor = "current_thread")]
async fn building_the_graph_allocates_linearly_in_the_number_of_beans() {
    // The first build in a process also pays for one-shot work (the tracing
    // subscriber install); keep it out of the measurements.
    let _ = build_chain!();

    let (with_16, _app) = chain_16!();
    let (with_32, _app) = chain_32!();
    let (with_64, _app) = chain_64!();

    let first_step = with_32 - with_16;
    let second_step = with_64 - with_32;

    assert!(
        first_step <= 16 * ALLOCATIONS_PER_BEAN,
        "16 → 32 beans cost {first_step} allocations \
         (budget: {ALLOCATIONS_PER_BEAN} per bean, {} in total)",
        16 * ALLOCATIONS_PER_BEAN
    );
    assert!(
        second_step <= 32 * ALLOCATIONS_PER_BEAN,
        "32 → 64 beans cost {second_step} allocations \
         (budget: {ALLOCATIONS_PER_BEAN} per bean, {} in total) — \
         16 → 32 cost {first_step}, so the build is no longer linear",
        32 * ALLOCATIONS_PER_BEAN
    );
}

#[r2e_core::test(flavor = "current_thread")]
async fn reading_a_bean_from_the_state_does_not_allocate() {
    let (_, app) = chain_64!();
    let state = app.state();

    let before = allocations();
    for _ in 0..100 {
        // Both ends of the list: the lookup cost must not depend on where
        // the bean sits in the state.
        black_box(state.get::<Root>().0);
        black_box(state.get::<C64>().0);
    }
    let total = allocations() - before;

    // The lookup is resolved at compile time; the only cost is the bean's
    // own `Clone`, an `Arc` bump here.
    assert_eq!(
        total, 0,
        "200 `state.get::<T>()` calls allocated {total} times: \
         reading a bean must not reach the allocator"
    );
}

// ── Clone accounting ───────────────────────────────────────────────────────

thread_local! {
    static TRACKED_CLONES: Cell<u64> = const { Cell::new(0) };
}

fn tracked_clones() -> u64 {
    TRACKED_CLONES.with(Cell::get)
}

/// A bean that counts how often the framework clones it.
struct Tracked;

impl Clone for Tracked {
    fn clone(&self) -> Self {
        TRACKED_CLONES.with(|count| count.set(count.get() + 1));
        Tracked
    }
}

macro_rules! dependents {
    ($($name:ident)*) => {$(
        #[derive(Clone)]
        struct $name;

        #[bean]
        impl $name {
            fn new(_tracked: Tracked) -> Self {
                Self
            }
        }
    )*};
}

dependents!(D1 D2 D3 D4);

/// Does not depend on `Tracked`: must not cause a clone of it.
#[derive(Clone)]
struct Unrelated;

#[bean]
impl Unrelated {
    fn new() -> Self {
        Self
    }
}

#[controller]
struct TrackedController {
    #[inject]
    tracked: Tracked,
}

#[routes]
impl TrackedController {
    #[get("/budget")]
    async fn handle(&self) -> StatusCode {
        black_box(&self.tracked);
        StatusCode::NO_CONTENT
    }
}

#[r2e_core::test(flavor = "current_thread")]
async fn a_bean_is_cloned_once_per_dependent_and_never_per_request() {
    // No dependent: only the copy that lands in the state — and a bean that
    // does not inject `Tracked` changes nothing.
    let before = tracked_clones();
    let _app = AppBuilder::new()
        .provide(Tracked)
        .register::<Unrelated>()
        .build_state()
        .await;
    let alone = tracked_clones() - before;
    assert!(
        alone <= CLONES_INTO_STATE,
        "a provided bean nobody injects was cloned {alone} times by build_state() \
         (budget: {CLONES_INTO_STATE})"
    );

    // Four dependents: one clone each on top of that.
    let before = tracked_clones();
    let app = AppBuilder::new()
        .provide(Tracked)
        .register::<D1>()
        .register::<D2>()
        .register::<D3>()
        .register::<D4>()
        .build_state()
        .await;
    let with_dependents = tracked_clones() - before;
    assert!(
        with_dependents <= 4 + CLONES_INTO_STATE,
        "a bean with 4 dependents was cloned {with_dependents} times by build_state() \
         (budget: one per dependent + {CLONES_INTO_STATE})"
    );

    // A controller keeps one copy in its core, taken at registration.
    let before = tracked_clones();
    let router = app.register_controller::<TrackedController>().build();
    let registration = tracked_clones() - before;
    assert!(
        registration <= CLONES_PER_CONTROLLER,
        "registering a controller cloned its injected bean {registration} times \
         (budget: {CLONES_PER_CONTROLLER})"
    );

    // Requests share the core: no DI work, no clone.
    let before = tracked_clones();
    allocations_per_request(&router).await;
    let per_request = tracked_clones() - before;
    assert_eq!(
        per_request, 0,
        "serving requests cloned an injected bean {per_request} times: \
         the controller core is being rebuilt, or the bean re-resolved, per request"
    );
}
