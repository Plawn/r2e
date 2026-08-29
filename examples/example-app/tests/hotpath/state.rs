//! The router state — the resolved bean HList — is cloned by the HTTP backend
//! on **every** request (`axum/src/handler/service.rs`, `Handler::call(handler,
//! req, self.state.clone())`), whether or not the handler declares `State<S>`.
//!
//! Before task #992 that state *was* the `HCons` chain itself, so the
//! per-request cost was the sum of every bean's `Clone`: O(N) in the size of
//! the bean graph. Beans are `Arc`-shaped by convention, so that was usually N
//! refcount bumps rather than N deep copies — but nothing enforced it, and N
//! grows with the app.
//!
//! Since #992 the state is `BeanState<L>` = one `Arc<L>`, so each of those
//! clones is a single refcount bump no matter how wide the graph is, and no
//! bean `Clone` runs at all. (The backend takes two state clones per request,
//! which is why the "before" figures below are 2N, not N — the guarantee is
//! O(1) in the bean count, not "one clone".)
//!
//! # What these tests assert
//!
//! Width invariance, in the same spirit as the config-size invariance the rest
//! of this target asserts: the same router is built over a narrow (8-bean) and
//! a wide (64-bean) state and the per-request cost must not grow. Two flavours
//! of bean, because the two halves of the finding are different:
//!
//! - [`ArcBean`] is the well-behaved bean (an `Arc` inside). It counts its own
//!   `Clone` calls, so the test sees the O(N) refcount traffic directly, which
//!   no allocation counter could show.
//! - [`OwnedBean`] is the bean nothing stopped a user from writing (a `String`
//!   inside). Its cost is visible to the allocation counter, and it is what
//!   made the old shape a latent deep-copy-per-request.
//!
//! Each property is asserted twice, over two different states:
//!
//! - against a hand-assembled `BeanState` + bare `Router::with_state`, which
//!   isolates the wrapper's own cost from everything the builder does;
//! - against the router `AppBuilder::build_state().build()` actually produces,
//!   which is the integration guard — it fails if the builder or `build_inner`
//!   ever unwraps the state on the way to `with_state`, however correct
//!   `BeanState` itself remains.
//!
//! # Reproducing the "before" numbers
//!
//! For the hand-assembled half, replace [`into_state`]/[`StateOf`] with the
//! identity (`type StateOf<L> = L; fn into_state<L>(l: L) -> L { l }`) — that
//! is exactly the pre-#992 state shape. For the builder half, make
//! `build_state()` return `<P as BuildHList>::Output` again. The figures are
//! recorded in `docs/claude/hot-path-clone-audit.md`.

use std::cell::Cell;
use std::sync::Arc;

use r2e::http::routing::get;
use r2e::http::{Body, Request, Router, StatusCode};
use r2e::type_list::{BeanState, HCons, HNil};
use tower::ServiceExt;

use crate::counter::{runtime, steady_state, Alloc};

const ITERATIONS: u64 = 200;

// ── The state shape under test ───────────────────────────────────────────

/// What `build_state()` installs as the router state, given the materialized
/// HList `L`. Swap this for the identity to measure the pre-#992 shape.
type StateOf<L> = BeanState<L>;

fn into_state<L>(list: L) -> StateOf<L> {
    BeanState::new(list)
}

// ── Beans ────────────────────────────────────────────────────────────────

thread_local! {
    /// Bean `Clone` calls on this thread. Counting them (rather than only
    /// allocations) is the point: an `Arc`-shaped bean's clone allocates
    /// nothing, so the O(N) refcount traffic is invisible to the allocator.
    static CLONES: Cell<u64> = const { Cell::new(0) };
}

fn bump() {
    CLONES.with(|c| c.set(c.get().wrapping_add(1)));
}

/// Run `f` `iterations` times to warm up, then `iterations` more, and report
/// bean clones per iteration.
fn clones_per_iteration(iterations: u64, mut f: impl FnMut()) -> u64 {
    for _ in 0..iterations {
        f();
    }
    let before = CLONES.with(Cell::get);
    for _ in 0..iterations {
        f();
    }
    (CLONES.with(Cell::get) - before) / iterations
}

/// The conventional bean: cheap to clone (one refcount bump), allocates
/// nothing. `N` only makes the types distinct — an HList slot is addressed by
/// type, so a state of 64 beans needs 64 types.
#[derive(Debug)]
pub struct ArcBean<const N: usize>(#[allow(dead_code)] Arc<str>);

impl<const N: usize> ArcBean<N> {
    fn new() -> Self {
        ArcBean(Arc::from("bean"))
    }
}

impl<const N: usize> Clone for ArcBean<N> {
    fn clone(&self) -> Self {
        bump();
        ArcBean(Arc::clone(&self.0))
    }
}

/// The bean nothing prevented: owns its data, so every clone is a deep copy.
/// Registering one of these was enough to make every request in the app pay a
/// heap allocation per bean, silently.
#[derive(Debug)]
pub struct OwnedBean<const N: usize>(#[allow(dead_code)] String);

impl<const N: usize> OwnedBean<N> {
    fn new() -> Self {
        // Long enough that a per-request deep copy is unmistakable in both the
        // allocation count and the byte volume.
        OwnedBean(format!("bean-{N:0>56}"))
    }
}

impl<const N: usize> Clone for OwnedBean<N> {
    fn clone(&self) -> Self {
        bump();
        OwnedBean(self.0.clone())
    }
}

// ── HList construction ───────────────────────────────────────────────────

macro_rules! hlist_ty {
    ($bean:ident;) => { HNil };
    ($bean:ident; $n:literal $(, $rest:literal)* $(,)?) => {
        HCons<$bean<$n>, hlist_ty!($bean; $($rest),*)>
    };
}

macro_rules! hlist_val {
    ($bean:ident;) => { HNil };
    ($bean:ident; $n:literal $(, $rest:literal)* $(,)?) => {
        HCons { head: <$bean<$n>>::new(), tail: hlist_val!($bean; $($rest),*) }
    };
}

/// Define an `ArcBean` state and an `OwnedBean` state of the same width.
macro_rules! define_states {
    ($arc_ty:ident, $arc_fn:ident, $owned_ty:ident, $owned_fn:ident; $($n:literal),+ $(,)?) => {
        type $arc_ty = StateOf<hlist_ty!(ArcBean; $($n),+)>;
        fn $arc_fn() -> $arc_ty { into_state(hlist_val!(ArcBean; $($n),+)) }

        type $owned_ty = StateOf<hlist_ty!(OwnedBean; $($n),+)>;
        fn $owned_fn() -> $owned_ty { into_state(hlist_val!(OwnedBean; $($n),+)) }
    };
}

define_states!(NarrowArc, narrow_arc, NarrowOwned, narrow_owned; 0, 1, 2, 3, 4, 5, 6, 7);

define_states!(
    WideArc, wide_arc, WideOwned, wide_owned;
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
    16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
    32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,
    48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63,
);

// ── Driving requests ─────────────────────────────────────────────────────

fn request() -> Request<Body> {
    Request::builder()
        .uri("/plain")
        .body(Body::empty())
        .expect("request")
}

/// A route that declares no `State<S>` at all — the backend clones the state
/// for it regardless, which is exactly why the state's `Clone` is a hot path.
fn router<S: Clone + Send + Sync + 'static>(state: S) -> Router {
    Router::new()
        .route("/plain", get(|| async { "ok" }))
        .with_state(state)
}

fn drive(rt: &r2e::rt::Runtime, router: &Router) {
    let response = rt
        .block_on(router.clone().oneshot(request()))
        .expect("infallible router");
    assert_eq!(response.status(), StatusCode::OK);
}

fn bean_clones<S: Clone + Send + Sync + 'static>(rt: &r2e::rt::Runtime, state: S) -> u64 {
    let router = router(state);
    clones_per_iteration(ITERATIONS, || drive(rt, &router))
}

fn allocations<S: Clone + Send + Sync + 'static>(rt: &r2e::rt::Runtime, state: S) -> Alloc {
    let router = router(state);
    steady_state(ITERATIONS, || drive(rt, &router))
}

// ── Guards ───────────────────────────────────────────────────────────────

/// The per-request state clone must not scale with the number of beans.
///
/// Pre-#992 this measured 16 bean clones on the narrow state and 128 on the
/// wide one — the backend clones the state twice per request, so two refcount
/// bumps per bean, per request, on every route. Now the state is a single
/// `Arc`, so no bean is cloned at all.
#[test]
fn state_clone_does_not_scale_with_graph_width() {
    let rt = runtime();

    let narrow = bean_clones(&rt, narrow_arc());
    let wide = bean_clones(&rt, wide_arc());

    eprintln!(
        "[hotpath] router state clone: 8 beans = {narrow} bean clones/request; \
         64 beans = {wide} bean clones/request"
    );

    assert_eq!(
        wide, narrow,
        "the per-request router-state clone scales with the size of the bean \
         graph ({narrow} bean clones at 8 beans -> {wide} at 64). The HList is \
         being cloned element-wise instead of shared behind one Arc — see \
         docs/claude/hot-path-clone-audit.md."
    );
    assert_eq!(
        wide, 0,
        "cloning the router state must not clone any bean: the state is one \
         `Arc<HList>` and its clone is a single refcount bump"
    );
}

/// The same property in allocation terms, for a bean that owns its data.
///
/// Nothing in the framework requires a bean to be `Arc`-shaped, so before #992
/// a graph of owning beans deep-copied all of them on every request. With the
/// HList behind one `Arc` the bean's own `Clone` never runs on the request
/// path, so its shape stops mattering.
#[test]
fn owning_beans_are_not_deep_copied_per_request() {
    let rt = runtime();

    let narrow = allocations(&rt, narrow_owned());
    let wide = allocations(&rt, wide_owned());

    eprintln!(
        "[hotpath] router state with owning beans: 8 beans = {narrow}; \
         64 beans = {wide} (per request)"
    );

    // A deep clone of the wide state costs 56 more allocations and >= 3.5 KiB
    // per request than the narrow one; the slack is two orders of magnitude
    // below that.
    assert!(
        wide.count <= narrow.count + 2,
        "per-request allocation COUNT grows with the number of beans in the \
         state ({} -> {}). The router state is deep-cloning its beans — see \
         docs/claude/hot-path-clone-audit.md.",
        narrow.count,
        wide.count,
    );
    assert!(
        wide.bytes <= narrow.bytes + 128,
        "per-request allocated BYTES grow with the number of beans in the \
         state ({} -> {}). The router state is deep-cloning its beans — see \
         docs/claude/hot-path-clone-audit.md.",
        narrow.bytes,
        wide.bytes,
    );
}

// ── The same property, through the real builder ──────────────────────────
//
// Everything above hand-assembles the state, which pins the *wrapper's* cost
// but not the integration: nothing there would notice if `build_state()` or
// `build_inner()` started handing the router a bare HList again. These two
// guards close that hole by measuring the router `AppBuilder::build_state()`
// actually produces — provisions in, `Router` out, no `BeanState` named by the
// test at all.

/// Chain one `.provide()` per bean onto a builder.
macro_rules! provide_beans {
    ($builder:expr; $bean:ident;) => { $builder };
    ($builder:expr; $bean:ident; $n:literal $(, $rest:literal)* $(,)?) => {
        provide_beans!($builder.provide(<$bean<$n>>::new()); $bean; $($rest),*)
    };
}

/// A route registered *before* the state is applied, so the backend clones the
/// builder's state for it — the whole point. It declares no `State<S>`, like
/// the majority of real handlers.
fn plain_route<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/plain", get(|| async { "ok" }))
}

/// Define `fn <name>() -> Router` building a router through the real builder
/// over a state of the given width.
macro_rules! define_built_routers {
    ($($name:ident, $bean:ident, [$($n:literal),+ $(,)?];)+) => {
        $(
            fn $name(rt: &r2e::rt::Runtime) -> Router {
                rt.block_on(async {
                    provide_beans!(r2e::AppBuilder::new(); $bean; $($n),+)
                        .build_state()
                        .await
                        .register_routes(plain_route())
                        .build()
                })
            }
        )+
    };
}

define_built_routers! {
    built_narrow_arc, ArcBean, [0, 1, 2, 3, 4, 5, 6, 7];
    built_wide_arc, ArcBean, [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
        16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
        32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,
        48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63,
    ];
    built_narrow_owned, OwnedBean, [0, 1, 2, 3, 4, 5, 6, 7];
    built_wide_owned, OwnedBean, [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
        16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
        32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,
        48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63,
    ];
}

/// End-to-end: the state the **builder** installs must not clone beans per
/// request either.
///
/// This is the integration guard for the whole path — `.provide()` →
/// `build_state()` → `AppBuilder::build()` → `Router::with_state`. Unwrapping
/// the `BeanState` anywhere along it (installing `(*state).clone()`, returning
/// the bare `<P as BuildHList>::Output` from `build_state()`, …) puts the
/// element-wise `HCons` clone back on the request path and fails here, even
/// though `BeanState` itself would still be correct.
#[test]
fn builder_router_state_clone_does_not_scale_with_graph_width() {
    let rt = runtime();

    // Build both routers first: `build_state()` clones each bean out of the
    // context once, and that is startup, not the request path.
    let narrow_router = built_narrow_arc(&rt);
    let wide_router = built_wide_arc(&rt);

    let narrow = clones_per_iteration(ITERATIONS, || drive(&rt, &narrow_router));
    let wide = clones_per_iteration(ITERATIONS, || drive(&rt, &wide_router));

    eprintln!(
        "[hotpath] built router state clone: 8 beans = {narrow} bean clones/request; \
         64 beans = {wide} bean clones/request"
    );

    assert_eq!(
        wide, narrow,
        "the per-request clone of the state `AppBuilder::build_state()` installs \
         scales with the size of the bean graph ({narrow} bean clones at 8 beans \
         -> {wide} at 64). The builder or `build_inner` is handing the router an \
         element-wise-cloned HList instead of the `BeanState` wrapper — see \
         docs/claude/hot-path-clone-audit.md."
    );
    assert_eq!(
        wide, 0,
        "cloning the state the builder installs must not clone any bean: \
         `build_state()` returns `BeanState<L>` and `build_inner` installs it \
         as-is"
    );
}

/// The allocation form of the same integration guard, for owning beans.
#[test]
fn builder_router_does_not_deep_copy_owning_beans_per_request() {
    let rt = runtime();

    let narrow_router = built_narrow_owned(&rt);
    let wide_router = built_wide_owned(&rt);

    let narrow = steady_state(ITERATIONS, || drive(&rt, &narrow_router));
    let wide = steady_state(ITERATIONS, || drive(&rt, &wide_router));

    eprintln!(
        "[hotpath] built router with owning beans: 8 beans = {narrow}; \
         64 beans = {wide} (per request)"
    );

    assert!(
        wide.count <= narrow.count + 2,
        "per-request allocation COUNT grows with the number of beans the builder \
         put in the state ({} -> {}). The installed router state is deep-cloning \
         its beans — see docs/claude/hot-path-clone-audit.md.",
        narrow.count,
        wide.count,
    );
    assert!(
        wide.bytes <= narrow.bytes + 128,
        "per-request allocated BYTES grow with the number of beans the builder \
         put in the state ({} -> {}). The installed router state is deep-cloning \
         its beans — see docs/claude/hot-path-clone-audit.md.",
        narrow.bytes,
        wide.bytes,
    );
}
