//! Built-in interceptors (`r2e-utils`).
//!
//! An interceptor is built once per route at registration and then invoked on
//! every intercepted request, so anything it clones or formats in `around` is a
//! per-request cost. Two shapes were fixed under this ticket:
//!
//! * eager `&format!(..)` of a log message before `tracing` decides whether the
//!   level is even enabled — paid on every request of a filtered-out level;
//! * `self.metric_name.clone()` / `self.group.clone()` of a `String` that is
//!   constant for the interceptor's whole life.
//!
//! No `tracing` subscriber is installed in this binary, so every level is
//! disabled: a correctly-lazy interceptor must then allocate nothing at all.

use r2e::r2e_utils::{Counted, Logged, MetricTimed, Timed};
use r2e::decorators::interceptors::{Interceptor, InterceptorContext};

use crate::counter::{assert_config_size_invariant, runtime, steady_state, Alloc};

const ITERATIONS: u64 = 500;

fn ctx() -> InterceptorContext {
    InterceptorContext {
        method_name: "handler",
        controller_name: "HotPathController",
    }
}

/// Cost of driving the bare payload with no interceptor: the floor that the
/// runtime's `block_on` itself contributes, so the assertions below measure the
/// interceptor and nothing else.
fn baseline(rt: &r2e::rt::Runtime) -> Alloc {
    steady_state(ITERATIONS, || {
        let out = rt.block_on(async { 7u32 });
        assert_eq!(out, 7);
    })
}

fn through<I: Interceptor<u32>>(rt: &r2e::rt::Runtime, interceptor: &I) -> Alloc {
    steady_state(ITERATIONS, || {
        let out = rt.block_on(interceptor.around(ctx(), || async { 7u32 }));
        assert_eq!(out, 7);
    })
}

/// The built-in interceptors must not format (or allocate) anything when the
/// target level is disabled. Measured against the no-interceptor floor so the
/// runtime's own overhead cannot mask a regression.
#[test]
fn logging_interceptors_allocate_nothing_when_the_level_is_disabled() {
    let rt = runtime();
    let floor = baseline(&rt);

    for (name, cost) in [
        ("Logged::info", through(&rt, &Logged::info())),
        ("Logged::debug", through(&rt, &Logged::debug())),
        // Threshold far above the (empty) handler's duration: nothing to log.
        ("Timed::threshold", through(&rt, &Timed::threshold(60_000))),
        ("Counted", through(&rt, &Counted::new("hot_path_calls"))),
        ("MetricTimed", through(&rt, &MetricTimed::new("hot_path_ms"))),
    ] {
        eprintln!("[hotpath] {name}: {cost} per call (floor: {floor})");
        assert!(
            cost.count <= floor.count && cost.bytes <= floor.bytes,
            "{name} allocates per call ({cost}) above the no-interceptor floor \
             ({floor}). A log message is being formatted before the level is \
             checked — see docs/claude/hot-path-clone-audit.md.",
        );
    }
}

/// `Counted`/`MetricTimed` hold a metric name that is fixed at construction.
/// Copying it per call would make the cost scale with the name's length.
#[test]
fn metric_names_are_not_copied_per_call() {
    let rt = runtime();
    let long = "a".repeat(4096);

    assert_config_size_invariant(
        "Counted::metric_name",
        through(&rt, &Counted::new("n")),
        through(&rt, &Counted::new(&long)),
        2,
        256,
    );
    assert_config_size_invariant(
        "MetricTimed::metric_name",
        through(&rt, &MetricTimed::new("n")),
        through(&rt, &MetricTimed::new(&long)),
        2,
        256,
    );
}
