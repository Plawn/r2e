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
//! Both halves of the logging story are measured. By default no `tracing`
//! subscriber is installed in this binary, so every level is disabled and a
//! correctly-lazy interceptor must allocate nothing at all; the tests at the
//! bottom install a subscriber that is enabled for everything and discards what
//! it receives, which is the case an eager `&format!(..)` could not survive.

use r2e::decorators::interceptors::{Interceptor, InterceptorContext};
use r2e::r2e_utils::interceptors::CacheInvalidateInterceptor;
use r2e::r2e_utils::{CacheInvalidate, Counted, Logged, MetricTimed, Timed};
use r2e::DecoratorSpec;

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

// ── The invalidation prefix ────────────────────────────────────────────────

/// Build the `CacheInvalidate` product the way registration does: resolve the
/// store bean, then hand the spec the resulting context.
fn invalidator(rt: &r2e::rt::Runtime, group: &str) -> CacheInvalidateInterceptor {
    let spec = CacheInvalidate::group(group);
    rt.block_on(async {
        let mut reg = r2e::beans::BeanRegistry::new();
        reg.provide(r2e::r2e_cache::InMemoryStore::shared());
        let ctx = reg.resolve().await.expect("resolve the cache store");
        spec.build(&ctx)
    })
}

/// `CacheInvalidate` appends the `:` separator to its group name once, at build
/// time, and shares the result as an `Arc<str>`. Doing it in `around` instead
/// would `format!` a fresh `String` on every invalidating request, sized by the
/// group name — so the guard is that the per-call cost does not move when the
/// group grows from one byte to four kilobytes.
#[test]
fn cache_invalidation_prefix_is_not_rebuilt_per_call() {
    let rt = runtime();
    let short = invalidator(&rt, "u");
    let long = invalidator(&rt, &"g".repeat(4096));

    assert_config_size_invariant(
        "CacheInvalidate::group",
        through(&rt, &short),
        through(&rt, &long),
        2,
        256,
    );
}

// ── The formatted path, with a subscriber that is actually enabled ─────────

/// A subscriber that says yes to everything and then throws the event away.
///
/// This is the case the `format_args!` rewrite exists for. With no subscriber
/// at all (the tests above) `tracing` short-circuits before the message is even
/// referenced, so an eager `&format!(..)` would *also* have to be optimised
/// away by the level check — the disabled tests cannot tell the two apart. Here
/// the event is dispatched for real; only the subscriber declines to render it.
/// A lazily-formatted message costs nothing on this path, an eagerly-formatted
/// one costs a `String` per request.
struct EnabledButDiscarding;

impl tracing::Subscriber for EnabledButDiscarding {
    /// `sometimes`, deliberately, and NOT the default `register_callsite`.
    ///
    /// The default one answers `Interest::always()` for any callsite this
    /// subscriber enables — and interest is cached per callsite for the whole
    /// *process*, so `always` would make the `tracing` macros skip the level
    /// check on every other thread too, silently converting the
    /// no-subscriber tests above into enabled ones depending on which test ran
    /// first. `sometimes` keeps the per-event `enabled()` call, which those
    /// threads answer through `NoSubscriber` (false), so the two halves of this
    /// file stay independent however `cargo test` interleaves them.
    fn register_callsite(
        &self,
        _: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::sometimes()
    }
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, _: &tracing::Event<'_>) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// With the level enabled, the interceptors must still not build the message on
/// the heap: `format_args!` renders into the subscriber, and a subscriber that
/// ignores the fields renders nothing at all.
#[test]
fn logging_interceptors_do_not_allocate_a_message_when_the_level_is_enabled() {
    let rt = runtime();
    tracing::subscriber::with_default(EnabledButDiscarding, || {
        let floor = baseline(&rt);

        for (name, cost) in [
            ("Logged::info", through(&rt, &Logged::info())),
            ("Timed::info", through(&rt, &Timed::info())),
            ("Counted", through(&rt, &Counted::new("hot_path_calls"))),
            (
                "MetricTimed",
                through(&rt, &MetricTimed::new("hot_path_ms")),
            ),
        ] {
            eprintln!("[hotpath] {name} (enabled): {cost} per call (floor: {floor})");
            assert!(
                cost.count <= floor.count && cost.bytes <= floor.bytes,
                "{name} allocates per call ({cost}) above the no-interceptor \
                 floor ({floor}) with the level enabled. The log message is \
                 being formatted into a String instead of passed as \
                 format_args! — see docs/claude/hot-path-clone-audit.md.",
            );
        }
    });
}

/// Same, but for the part of the message that comes from the interceptor's own
/// configuration: an enabled level must not turn the metric name into a
/// per-call copy either.
#[test]
fn metric_names_are_not_copied_per_call_when_the_level_is_enabled() {
    let rt = runtime();
    let long = "a".repeat(4096);

    tracing::subscriber::with_default(EnabledButDiscarding, || {
        assert_config_size_invariant(
            "Counted::metric_name (enabled)",
            through(&rt, &Counted::new("n")),
            through(&rt, &Counted::new(&long)),
            2,
            256,
        );
        assert_config_size_invariant(
            "MetricTimed::metric_name (enabled)",
            through(&rt, &MetricTimed::new("n")),
            through(&rt, &MetricTimed::new(&long)),
            2,
            256,
        );
    });
}
