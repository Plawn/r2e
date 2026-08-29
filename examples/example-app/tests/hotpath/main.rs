//! Per-request allocation guard for the framework's hot-path wrappers
//! (task #982, `docs/claude/hot-path-clone-audit.md`).
//!
//! The audit's invariant is:
//!
//! > Configuration is `Arc`'d once at layer/plugin/decorator build time.
//! > Everything a per-request path clones is an `Arc`, a `Copy`, or genuine
//! > per-request data.
//!
//! A plain unit test cannot see that invariant break: the regression the ticket
//! started from (`PrometheusService::clone` deep-copying `exclude_paths`) is
//! *behaviourally* invisible — same responses, same metrics, just one
//! `Vec<String>` clone per request. So this target installs a counting
//! [`#[global_allocator]`](counter::CountingAllocator) and measures the
//! allocations a request actually performs.
//!
//! # Why the assertions are size-invariance, not absolute numbers
//!
//! An absolute per-request budget is machine- and dependency-version-specific
//! and rots fast. Every guard here instead builds the SAME wrapper twice —
//! once with a small immutable config, once with a deliberately large one —
//! and asserts the per-request allocation count and byte volume do not grow
//! with the config. That is exactly the property `Arc`-ing the config buys,
//! it is independent of the machine, and it fails hard the moment someone
//! reintroduces a by-value config field: a deep clone makes the cost scale
//! with the config, which is the whole bug class.
//!
//! `layers::composed_stack_budget` additionally prints (and loosely bounds) the
//! absolute per-request figure for the full wrapper stack, so the number is
//! *visible* in CI output as the ticket asks — see that test for how to
//! re-baseline it.
//!
//! # Determinism
//!
//! The counter is a thread-local, and every measurement drives its requests to
//! completion on the test's own thread through a `current_thread` runtime, so
//! concurrently-running tests in this binary cannot contaminate each other.
//! Each measurement warms up first (global metric registration, lazily-built
//! label series, `OnceLock`s) before counting.

#[global_allocator]
static GLOBAL: counter::CountingAllocator = counter::CountingAllocator;

mod accounting;
mod counter;
mod decorators;
mod jwt;
mod layers;
mod openapi;
mod state;
