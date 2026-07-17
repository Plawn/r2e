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
//! * a call to [`turn`] as the very first statement of the async test body,
//!   before any [`crate::TestApp`] boot.
//!
//! [`turn`] blocks until every **registered** order strictly lower than its own
//! (in the same group) has *completed*. Orders need not be contiguous — the
//! inventory registry is the source of truth for which orders exist, so
//! `(10, 20, 30)` works exactly like `(1, 2, 3)`. Tests carrying no `order`
//! never call [`turn`] and stay fully parallel.
//!
//! The returned [`TurnGuard`] must be held for the duration of the test. Its
//! `Drop` marks the order completed and wakes the next waiter — including on
//! panic (unwind), so a failing ordered test can never deadlock its group.
//!
//! # Fail-fast
//!
//! If an ordered test panics, its group is *poisoned* (first poisoner wins):
//! every later order in that group panics immediately from [`turn`] instead of
//! running, reporting which predecessor failed.
//!
//! # Watchdog
//!
//! While waiting, if the group makes no progress for
//! `R2E_TEST_ORDER_TIMEOUT_SECS` seconds (default 60, read once), [`turn`]
//! panics with a diagnostic listing the pending lower orders — distinguishing
//! orders that were *registered but never started* (likely filtered out by a
//! `cargo test <filter>`, or starved by `--test-threads`) from orders that
//! *started but never finished*. This turns would-be deadlocks into loud,
//! actionable failures.
//!
//! # Global state & validation
//!
//! Group state is process-global (a [`std::sync::OnceLock`] map of per-group
//! cells), because a test binary runs all of its tests in one process. Each
//! group owns a [`std::sync::Mutex`] over its bookkeeping plus a
//! [`tokio::sync::Notify`] for wakeups, so [`turn`] works across the
//! independent Tokio runtimes that different tests spin up.
//!
//! Registry validation (duplicate `(group, order)` detection) is scoped **per
//! group** and runs at the start of every [`turn`] call: it only inspects the
//! entries of the group being entered, so a duplicate in one group surfaces as
//! a panic on that group's tests without poisoning unrelated groups.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

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

/// Per-group bookkeeping guarded by a [`std::sync::Mutex`].
struct GroupState {
    /// Orders that have finished (guard dropped), successfully or not.
    completed: HashSet<u32>,
    /// Orders that acquired their turn (started running).
    started: HashSet<u32>,
    /// First order to panic, plus its test path. Poisons all higher orders.
    poisoned: Option<(u32, &'static str)>,
    /// Updated on every start and every completion; drives the watchdog.
    last_progress: Instant,
}

/// One group's synchronization cell: std-mutex state + a tokio notifier.
struct GroupCell {
    state: Mutex<GroupState>,
    notify: Notify,
}

impl GroupCell {
    fn new() -> Self {
        Self {
            state: Mutex::new(GroupState {
                completed: HashSet::new(),
                started: HashSet::new(),
                poisoned: None,
                last_progress: Instant::now(),
            }),
            notify: Notify::new(),
        }
    }
}

type GroupMap = HashMap<&'static str, std::sync::Arc<GroupCell>>;

fn groups() -> &'static Mutex<GroupMap> {
    static GROUPS: OnceLock<Mutex<GroupMap>> = OnceLock::new();
    GROUPS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn group_cell(group: &'static str) -> std::sync::Arc<GroupCell> {
    let mut map = groups().lock().unwrap();
    map.entry(group).or_insert_with(|| std::sync::Arc::new(GroupCell::new())).clone()
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

/// Validate this group's slice of the registry: two entries with the same
/// `(group, order)` are a hard error. Panics with a clear message naming the
/// group, order, and both test paths. Scoped per group so it never poisons
/// unrelated groups.
fn validate_group(group: &'static str) {
    let mut seen: HashMap<u32, &'static str> = HashMap::new();
    for entry in inventory::iter::<OrderedTestEntry> {
        if entry.group != group {
            continue;
        }
        if let Some(prev) = seen.insert(entry.order, entry.test) {
            if prev != entry.test {
                panic!(
                    "duplicate ordered test registration in group '{}': order {} is claimed by \
                     both '{}' and '{}'. Each (group, order) must be unique.",
                    group, entry.order, prev, entry.test
                );
            }
        }
    }
}

/// Every registered order strictly below `order` in `group`, with its test path.
fn registered_lower_orders(group: &'static str, order: u32) -> Vec<(u32, &'static str)> {
    let mut lower: Vec<(u32, &'static str)> = inventory::iter::<OrderedTestEntry>
        .into_iter()
        .filter(|e| e.group == group && e.order < order)
        .map(|e| (e.order, e.test))
        .collect();
    lower.sort_by_key(|&(o, _)| o);
    lower.dedup_by_key(|&mut (o, _)| o);
    lower
}

/// Wait for this ordered test's turn in `group`.
///
/// Blocks until every registered order lower than `order` in the same group has
/// completed, then returns a [`TurnGuard`] that must be held for the rest of
/// the test. See the [module docs](self) for the full model, fail-fast, and
/// watchdog behaviour.
///
/// # Panics
///
/// * if a lower order in the group panicked (fail-fast: this test is skipped);
/// * if the group makes no progress within the watchdog timeout;
/// * if the registry contains a duplicate `(group, order)`.
pub async fn turn(group: &'static str, order: u32, test: &'static str) -> TurnGuard {
    turn_inner(group, order, test, env_timeout()).await
}

/// Like [`turn`] but with an explicit watchdog timeout. Test-only escape hatch
/// so the watchdog can be exercised without mutating the process-global env.
#[doc(hidden)]
pub async fn turn_with_timeout(
    group: &'static str,
    order: u32,
    test: &'static str,
    timeout: Duration,
) -> TurnGuard {
    turn_inner(group, order, test, timeout).await
}

async fn turn_inner(
    group: &'static str,
    order: u32,
    test: &'static str,
    timeout: Duration,
) -> TurnGuard {
    validate_group(group);

    let cell = group_cell(group);
    let lower = registered_lower_orders(group, order);

    loop {
        // Arm the notification BEFORE inspecting state, so a wakeup that races
        // with our state check is not lost (missed-wakeup safety).
        let notified = cell.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let progress_at = {
            let st = cell.state.lock().unwrap();

            // Fail-fast: a lower order already failed → skip this test.
            if let Some((porder, ptest)) = st.poisoned {
                if porder < order {
                    panic!(
                        "ordered test '{}' (group '{}', order {}) skipped: its ordered \
                         predecessor '{}' (order {}) failed.",
                        test, group, order, ptest, porder
                    );
                }
            }

            // Our turn iff every registered lower order has completed.
            if lower.iter().all(|(o, _)| st.completed.contains(o)) {
                let mut st = st;
                st.started.insert(order);
                st.last_progress = Instant::now();
                drop(st);
                return TurnGuard { order, test, cell: cell.clone() };
            }

            st.last_progress
        };

        // Watchdog: no group progress within the timeout ⇒ loud failure.
        let elapsed = progress_at.elapsed();
        if elapsed >= timeout {
            panic!("{}", watchdog_message(group, order, test, &lower, &cell, timeout));
        }

        // Wake on the next completion, or re-check when the remaining budget
        // elapses. Never hold the std Mutex across this await.
        let remaining = timeout - elapsed;
        let _ = tokio::time::timeout(remaining, notified).await;
    }
}

/// Build the watchdog diagnostic listing pending lower orders and their state.
fn watchdog_message(
    group: &'static str,
    order: u32,
    test: &'static str,
    lower: &[(u32, &'static str)],
    cell: &GroupCell,
    timeout: Duration,
) -> String {
    let st = cell.state.lock().unwrap();
    let mut never_started = Vec::new();
    let mut in_flight = Vec::new();
    for &(o, t) in lower {
        if st.completed.contains(&o) {
            continue;
        }
        if st.started.contains(&o) {
            in_flight.push(format!("  - order {} ('{}'): started but never finished", o, t));
        } else {
            never_started.push(format!(
                "  - order {} ('{}'): registered but never started (likely filtered out by a \
                 `cargo test <filter>`, or starved by `--test-threads`)",
                o, t
            ));
        }
    }
    drop(st);

    let mut msg = format!(
        "ordered test '{}' (group '{}', order {}) timed out after {}s waiting for its \
         predecessors. Pending lower orders:\n",
        test,
        group,
        order,
        timeout.as_secs()
    );
    for line in never_started.into_iter().chain(in_flight) {
        msg.push_str(&line);
        msg.push('\n');
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
    cell: std::sync::Arc<GroupCell>,
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        {
            let mut st = self.cell.state.lock().unwrap();
            st.completed.insert(self.order);
            st.last_progress = Instant::now();
            if std::thread::panicking() && st.poisoned.is_none() {
                st.poisoned = Some((self.order, self.test));
            }
        }
        // Wake every current waiter; each re-checks state under the lock.
        self.cell.notify.notify_waiters();
    }
}
