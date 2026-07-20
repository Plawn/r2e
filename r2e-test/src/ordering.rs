//! Ordered-test barrier for `#[r2e::test(..., order = N, group = "…")]`.
//!
//! Some integration tests must run one-at-a-time in a deterministic order
//! (e.g. a migration test that seeds state a later test reads). This module
//! provides the runtime barrier that makes that possible *within a single test
//! binary*, while leaving all other tests free to run in parallel.
//!
//! # Model
//!
//! Each ordered test is compiled with two things emitted by the proc macro:
//!
//! * an item-level [`inventory::submit!`] of an [`OrderedTestEntry`], so the
//!   full set of `(group, order, test)` triples that *exist* in the binary is
//!   known at startup (via [`inventory::collect!`]); and
//! * a call to [`turn`] as the very first statement of the test body, before
//!   any [`crate::TestApp`] boot.
//!
//! [`turn`] blocks until every **registered** order strictly lower than its own
//! (in the same group) has *completed*. Orders need not be contiguous — the
//! inventory registry is the source of truth for which orders exist, so
//! `(10, 20, 30)` works exactly like `(1, 2, 3)`. Tests carrying no `order`
//! never call [`turn`] and stay fully parallel.
//!
//! [`turn`] blocks its thread (plain [`std::sync::Condvar`], real-time clock).
//! That is deliberate: it runs as the first statement of the test, before
//! anything is spawned on the test's runtime, so there is nothing to yield to —
//! and it keeps the barrier immune to Tokio's virtual clock
//! (`start_paused = true`).
//!
//! The returned [`TurnGuard`] must be held for the duration of the test. Its
//! `Drop` marks the order completed and wakes the waiters — including on
//! panic (unwind), so a failing ordered test can never deadlock its group.
//!
//! # Fail-fast
//!
//! An ordered test that fails — by panicking, or by returning `Err` from a
//! `Result` test (the macro reports that via [`TurnGuard::mark_failed`]) —
//! *poisons* its group (first poisoner wins): every later order in that group
//! panics immediately from [`turn`] instead of running, reporting which
//! predecessor failed. A `#[should_panic]` ordered test does **not** poison its
//! group when it panics (the macro calls [`TurnGuard::expect_panic`]): the
//! expected panic is a pass.
//!
//! # Watchdog
//!
//! A *running* predecessor is never a timeout: as long as some lower order of
//! the group is started-but-unfinished, waiters park unconditionally (a hung
//! predecessor hangs the suite exactly like any hung test would). The watchdog
//! only arms while the group is idle and some lower order was *never started*
//! — the situation that would otherwise deadlock, typically a lower order
//! filtered out by `cargo test <filter>` or starved by `--test-threads`. After
//! `R2E_TEST_ORDER_TIMEOUT_SECS` seconds (default 60, read once) without group
//! progress, [`turn`] panics with a diagnostic listing the missing orders.
//!
//! # Global state & validation
//!
//! Group state is process-global (a [`std::sync::OnceLock`] map of per-group
//! cells), because a test binary runs all of its tests in one process. The
//! group's slice of the registry is scanned, validated, and cached once, when
//! the group's cell is first created: a duplicate `(group, order)` panics on
//! that group's tests without poisoning unrelated groups. All state locks
//! recover from [`std::sync::PoisonError`] (the bookkeeping is always left
//! consistent), so one test's unwind never degrades another test's diagnostic.

use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

pub use inventory;

/// A registered ordered test: one entry is submitted per `#[r2e::test]` that
/// carries an `order`. Collected process-wide via [`inventory`].
///
/// `test` is the fully-qualified path (`module_path!() + "::" + fn_name`) used
/// only for diagnostics.
pub struct OrderedTestEntry {
    /// Ordering group. Empty string `""` is the default group used when the
    /// `group = "…"` argument is omitted.
    pub group: &'static str,
    /// Position within the group. Ordered tests run in ascending `order`.
    pub order: u32,
    /// Fully-qualified test path, for diagnostics.
    pub test: &'static str,
}

inventory::collect!(OrderedTestEntry);

/// Outcome of a test body, as seen by the ordering barrier. Implemented for
/// `()` (infallible tests) and `Result` (an `Err` is a failed test). Used by
/// the macro expansion to poison the group on `Err` — not public API.
#[doc(hidden)]
pub trait TestOutcome {
    fn is_failed(&self) -> bool;
}

impl TestOutcome for () {
    fn is_failed(&self) -> bool {
        false
    }
}

impl<T, E> TestOutcome for Result<T, E> {
    fn is_failed(&self) -> bool {
        self.is_err()
    }
}

/// Per-group bookkeeping guarded by a [`std::sync::Mutex`].
struct GroupState {
    /// Orders that have finished (guard dropped), successfully or not.
    completed: HashSet<u32>,
    /// Orders that acquired their turn (started running).
    started: HashSet<u32>,
    /// First order to fail, plus its test path. Poisons all higher orders.
    poisoned: Option<(u32, &'static str)>,
    /// Updated on every start and every completion; drives the watchdog.
    last_progress: Instant,
}

/// One group's synchronization cell: its validated registry slice plus the
/// mutex/condvar pair the barrier parks on.
struct GroupCell {
    /// This group's `(order, test)` entries, sorted ascending, validated
    /// duplicate-free at cell creation.
    entries: Vec<(u32, &'static str)>,
    state: Mutex<GroupState>,
    condvar: Condvar,
}

/// The bookkeeping is always left consistent (state transitions are single
/// insertions), so a panic while holding the lock — which we avoid anyway —
/// must not degrade other tests' diagnostics into `PoisonError` unwraps.
fn recover<T>(result: Result<T, PoisonError<T>>) -> T {
    result.unwrap_or_else(PoisonError::into_inner)
}

impl GroupCell {
    /// Build the cell for `group`: scan the group's slice of the inventory
    /// registry once, panicking on duplicate `(group, order)` claims.
    fn new(group: &'static str) -> Self {
        let mut entries: Vec<(u32, &'static str)> = inventory::iter::<OrderedTestEntry>
            .into_iter()
            .filter(|e| e.group == group)
            .map(|e| (e.order, e.test))
            .collect();
        entries.sort_by_key(|&(order, _)| order);
        for pair in entries.windows(2) {
            let (order, first) = pair[0];
            let (next_order, second) = pair[1];
            if order == next_order {
                panic!(
                    "duplicate ordered test registration in group '{}': order {} is claimed by \
                     both '{}' and '{}'. Each (group, order) must be unique.",
                    group, order, first, second
                );
            }
        }
        Self {
            entries,
            state: Mutex::new(GroupState {
                completed: HashSet::new(),
                started: HashSet::new(),
                poisoned: None,
                last_progress: Instant::now(),
            }),
            condvar: Condvar::new(),
        }
    }
}

type GroupMap = std::collections::HashMap<&'static str, Arc<GroupCell>>;

fn groups() -> &'static Mutex<GroupMap> {
    static GROUPS: OnceLock<Mutex<GroupMap>> = OnceLock::new();
    GROUPS.get_or_init(|| Mutex::new(GroupMap::new()))
}

fn group_cell(group: &'static str) -> Arc<GroupCell> {
    let mut map = recover(groups().lock());
    map.entry(group)
        .or_insert_with(|| Arc::new(GroupCell::new(group)))
        .clone()
}

/// Default watchdog timeout, read once from `R2E_TEST_ORDER_TIMEOUT_SECS`.
fn env_timeout() -> Duration {
    static TIMEOUT: OnceLock<Duration> = OnceLock::new();
    *TIMEOUT.get_or_init(|| {
        let secs = std::env::var("R2E_TEST_ORDER_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(60);
        Duration::from_secs(secs)
    })
}

/// Wait for this ordered test's turn in `group`.
///
/// Blocks the calling thread until every registered order lower than `order`
/// in the same group has completed, then returns a [`TurnGuard`] that must be
/// held for the rest of the test. Must be the first statement of the test —
/// the macro guarantees this. See the [module docs](self) for the full model,
/// fail-fast, and watchdog behaviour.
///
/// # Panics
///
/// * if a lower order in the group failed (fail-fast: this test is skipped);
/// * if some lower order was never started and the group stays idle for the
///   watchdog timeout;
/// * if the registry contains a duplicate `(group, order)`.
pub fn turn(group: &'static str, order: u32, test: &'static str) -> TurnGuard {
    turn_inner(group, order, test, env_timeout())
}

/// Like [`turn`] but with an explicit watchdog timeout. Test-only escape hatch
/// so the watchdog can be exercised without mutating the process-global env.
#[doc(hidden)]
pub fn turn_with_timeout(
    group: &'static str,
    order: u32,
    test: &'static str,
    timeout: Duration,
) -> TurnGuard {
    turn_inner(group, order, test, timeout)
}

fn turn_inner(group: &'static str, order: u32, test: &'static str, timeout: Duration) -> TurnGuard {
    let cell = group_cell(group);
    let lower: Vec<(u32, &'static str)> = cell
        .entries
        .iter()
        .copied()
        .filter(|&(o, _)| o < order)
        .collect();

    let mut st = recover(cell.state.lock());
    loop {
        // Fail-fast: a lower order already failed → skip this test. The panic
        // must not fire while holding the lock (it would poison the mutex and
        // turn later diagnostics into opaque PoisonErrors).
        if let Some((porder, ptest)) = st.poisoned {
            if porder < order {
                drop(st);
                panic!(
                    "ordered test '{}' (group '{}', order {}) skipped: its ordered \
                     predecessor '{}' (order {}) failed.",
                    test, group, order, ptest, porder
                );
            }
        }

        // Our turn iff every registered lower order has completed.
        if lower.iter().all(|(o, _)| st.completed.contains(o)) {
            st.started.insert(order);
            st.last_progress = Instant::now();
            drop(st);
            return TurnGuard {
                order,
                test,
                cell: cell.clone(),
                failed: false,
                panic_expected: false,
            };
        }

        // A running predecessor is progress by definition: park without a
        // deadline (a hung predecessor hangs the suite like any hung test).
        // The watchdog only arms while the group is idle with never-started
        // lower orders outstanding — the would-be-deadlock case.
        let in_flight = lower
            .iter()
            .any(|(o, _)| st.started.contains(o) && !st.completed.contains(o));
        if in_flight {
            st = recover(cell.condvar.wait(st));
            continue;
        }

        let elapsed = st.last_progress.elapsed();
        if elapsed >= timeout {
            let message = watchdog_message(group, order, test, &lower, &st, timeout);
            drop(st);
            panic!("{}", message);
        }
        st = recover(cell.condvar.wait_timeout(st, timeout - elapsed)).0;
    }
}

/// Build the watchdog diagnostic listing pending lower orders and their state.
fn watchdog_message(
    group: &'static str,
    order: u32,
    test: &'static str,
    lower: &[(u32, &'static str)],
    st: &MutexGuard<'_, GroupState>,
    timeout: Duration,
) -> String {
    let mut msg = format!(
        "ordered test '{}' (group '{}', order {}) timed out after {}s waiting for its \
         predecessors. Pending lower orders:\n",
        test,
        group,
        order,
        timeout.as_secs()
    );
    for &(o, t) in lower {
        if st.completed.contains(&o) {
            continue;
        }
        if st.started.contains(&o) {
            msg.push_str(&format!(
                "  - order {} ('{}'): started but never finished\n",
                o, t
            ));
        } else {
            msg.push_str(&format!(
                "  - order {} ('{}'): registered but never started (likely filtered out by a \
                 `cargo test <filter>`, or starved by `--test-threads`)\n",
                o, t
            ));
        }
    }
    msg
}

/// Held for the duration of an ordered test. Dropping it marks the order
/// completed and releases the next waiter — including on panic, so a failing
/// ordered test poisons its group (fail-fast) rather than deadlocking it.
#[must_use = "the turn guard must be held for the whole test; dropping it early releases the next ordered test"]
pub struct TurnGuard {
    order: u32,
    test: &'static str,
    cell: Arc<GroupCell>,
    failed: bool,
    panic_expected: bool,
}

impl TurnGuard {
    /// Record that the test body failed without panicking (an `Err` from a
    /// `Result` test). Poisons the group on drop. Called by the macro
    /// expansion — not public API.
    #[doc(hidden)]
    pub fn mark_failed(&mut self) {
        self.failed = true;
    }

    /// Declare that a panic is this test's *success* path (`#[should_panic]`):
    /// an unwind must not poison the group. Called by the macro expansion —
    /// not public API.
    #[doc(hidden)]
    pub fn expect_panic(&mut self) {
        self.panic_expected = true;
    }
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        let panicked = std::thread::panicking() && !self.panic_expected;
        {
            let mut st = recover(self.cell.state.lock());
            st.completed.insert(self.order);
            st.last_progress = Instant::now();
            if (self.failed || panicked) && st.poisoned.is_none() {
                st.poisoned = Some((self.order, self.test));
            }
        }
        // Wake every waiter; each re-checks state under the lock.
        self.cell.condvar.notify_all();
    }
}
