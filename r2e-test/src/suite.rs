//! Runtime support for `#[r2e::test_suite]`.
//!
//! # One runtime per suite
//!
//! A suite amortises setup: `#[before_all]` builds a value that every `#[case]`
//! then reuses. That value routinely owns runtime-bound resources — a `TestApp`,
//! a database pool, a listening socket, a spawned background task. Such a
//! resource is registered with *one* reactor; when that reactor goes away the
//! resource does not error, it simply stops waking, and the failure surfaces
//! much later as an unexplained timeout.
//!
//! So the runtime is owned by the [`SuiteCell`] and lives exactly as long as the
//! suite value: both sit in the same `OnceLock` in the generated suite module,
//! so `#[after_all]` still has a live reactor under it. Every hook and every
//! case `block_on`s that same runtime.
//!
//! # …and it is shut down when the suite ends
//!
//! The `OnceLock` itself is never dropped, so the runtime cannot be reclaimed by
//! going out of scope. Instead the last case to finish runs
//! [`SuiteRuntime::finish`]: it drops the suite value *inside* the runtime (a
//! socket or a pool wants its driver present in `Drop`) and then shuts the
//! runtime down, so a suite's worker threads and detached tasks do not outlive
//! it and keep touching shared state while unrelated tests run. A case that
//! somehow arrives after that gets a named panic from
//! [`SuiteRuntime::get`] rather than a resource that quietly never wakes.
//!
//! [`SuiteCell::assert_on_suite_runtime`] is the guard-rail: every phase —
//! `#[before_all]`, each case, `#[after_each]`, `#[after_all]` — checks, from
//! inside its `block_on`, that it really is on the suite runtime, and panics
//! naming the two runtimes if it is not, rather than letting the suite degrade
//! into a pool timeout that looks like an infrastructure problem.

use std::fmt::Debug;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use r2e_core::rt::{Runtime, RuntimeHandle, RuntimeId};

/// How long suite teardown waits for `spawn_blocking` work before abandoning
/// the runtime's threads. Async tasks are dropped immediately either way; this
/// only bounds blocking work, which a test suite normally has none of.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

/// Shared state for one generated test suite, plus the runtime every case runs
/// on. See the [module docs](self) for why the runtime lives here.
#[doc(hidden)]
pub struct SuiteCell<T> {
    total_cases: usize,
    runtime_id: RuntimeId,
    /// `None` once the suite has been torn down — see [`SuiteRuntime::finish`].
    runtime: Mutex<Option<Runtime>>,
    state: Mutex<SuiteState<T>>,
}

/// Mutable suite bookkeeping guarded by [`SuiteCell`].
#[doc(hidden)]
pub struct SuiteState<T> {
    pub suite: Option<T>,
    pub init_failed: bool,
    pub completed_cases: usize,
    pub after_all_ran: bool,
}

impl<T> SuiteState<T> {
    /// Record one finished case and answer whether it is the one that must run
    /// `#[after_all]` and tear the suite down.
    ///
    /// The trigger is "every generated case has completed". libtest does not
    /// expose which cases the current process selected, so a filtered run
    /// (`cargo test <filter>`) legitimately never reaches it — documented, and
    /// the reason `#[ignore]` on a `#[case]` is a compile error.
    #[doc(hidden)]
    pub fn complete_case(&mut self, total_cases: usize) -> bool {
        self.completed_cases += 1;
        let last = self.completed_cases >= total_cases && !self.after_all_ran;
        if last {
            self.after_all_ran = true;
        }
        last
    }
}

impl<T> SuiteCell<T> {
    /// Create a suite cell for `total_cases` generated `#[case]` tests, taking
    /// ownership of the runtime the whole suite will run on.
    #[doc(hidden)]
    pub fn new(total_cases: usize, runtime: Runtime) -> Self {
        let runtime_id = runtime.id();
        Self {
            total_cases,
            runtime_id,
            runtime: Mutex::new(Some(runtime)),
            state: Mutex::new(SuiteState {
                suite: None,
                init_failed: false,
                completed_cases: 0,
                after_all_ran: false,
            }),
        }
    }

    /// Claim the suite runtime for the duration of one case.
    ///
    /// The returned guard holds the runtime slot, so nothing can shut the
    /// runtime down underneath a case that is still using it. Take it *before*
    /// [`lock`](Self::lock) — that order is fixed everywhere, so the two locks
    /// cannot deadlock against each other.
    #[doc(hidden)]
    pub fn runtime(&self) -> SuiteRuntime<'_> {
        SuiteRuntime(recover(self.runtime.lock()))
    }

    /// Lock the suite state, recovering from panic-induced mutex poisoning.
    #[doc(hidden)]
    pub fn lock(&self) -> MutexGuard<'_, SuiteState<T>> {
        recover(self.state.lock())
    }

    /// Number of generated `#[case]` tests in this suite.
    #[doc(hidden)]
    pub fn total_cases(&self) -> usize {
        self.total_cases
    }

    /// The identity of the suite runtime, live or already shut down.
    #[doc(hidden)]
    pub fn runtime_id(&self) -> RuntimeId {
        self.runtime_id
    }

    /// Guard-rail: assert the caller is running on the suite runtime.
    ///
    /// Called from inside every phase's `block_on` (`#[before_all]`, the case,
    /// `#[after_each]`, `#[after_all]`); `phase` names which one. It cannot
    /// fail as long as the generated code goes through
    /// [`runtime`](Self::runtime) — which is the point: if that ever regresses,
    /// the suite fails here, naming the cause, instead of hanging on a resource
    /// whose reactor is gone.
    ///
    /// # Panics
    ///
    /// If the calling thread is not driven by the suite runtime.
    #[doc(hidden)]
    pub fn assert_on_suite_runtime(&self, suite: &str, phase: &str) {
        let expected = self.runtime_id;
        match RuntimeHandle::try_current() {
            Some(handle) if handle.id() == expected => {}
            Some(handle) => panic!(
                "R2E test suite `{suite}`: `{phase}` is running on runtime {} but the suite \
                 state was built on runtime {expected}. Everything the suite holds that is bound \
                 to a reactor (a TestApp, a database pool, spawned tasks, timers) is registered \
                 with the suite runtime and would silently stop waking here — typically surfacing \
                 as a timeout, not as an error. Every case must run on the suite runtime \
                 (`SuiteCell::runtime`); this is a `#[r2e::test_suite]` bug, please report it.",
                handle.id()
            ),
            None => panic!(
                "R2E test suite `{suite}`: `{phase}` is not running on any runtime, so the \
                 suite's runtime-bound resources (a TestApp, a database pool, spawned tasks, \
                 timers) cannot make progress. Every case must run on the suite runtime \
                 (`SuiteCell::runtime`, id {expected}); this is a `#[r2e::test_suite]` bug, \
                 please report it."
            ),
        }
    }
}

/// The suite runtime, claimed for one case.
///
/// Held for the whole of a generated `#[test]`, so the runtime cannot be shut
/// down while a case is still driving work on it.
#[doc(hidden)]
pub struct SuiteRuntime<'a>(MutexGuard<'a, Option<Runtime>>);

impl SuiteRuntime<'_> {
    /// The suite runtime.
    ///
    /// # Panics
    ///
    /// If the suite has already been torn down — naming the suite and the
    /// phase, because the alternative (a resource with no reactor) is an
    /// unexplained hang. Under normal libtest scheduling this cannot happen:
    /// teardown only runs once every generated case has completed.
    #[doc(hidden)]
    pub fn get(&self, suite: &str, phase: &str) -> &Runtime {
        self.0.as_ref().unwrap_or_else(|| {
            panic!(
                "R2E test suite `{suite}`: `{phase}` ran after the suite was torn down. \
                 `#[after_all]` runs when the last generated case finishes, and it drops the \
                 suite value and shuts the suite runtime down; nothing may use the suite \
                 afterwards. This is a `#[r2e::test_suite]` bug, please report it."
            )
        })
    }

    /// Whether the suite runtime is still alive (false after [`finish`](Self::finish)).
    #[doc(hidden)]
    pub fn is_live(&self) -> bool {
        self.0.is_some()
    }

    /// End the suite: drop `value` inside the runtime, then shut the runtime
    /// down.
    ///
    /// The suite value is dropped *on* the runtime on purpose — a socket, a
    /// pool or a `TestApp` may deregister from the I/O driver in `Drop`, and
    /// dropping it after the reactor is gone is exactly the failure mode this
    /// whole module exists to avoid. After this the slot is empty and any later
    /// [`get`](Self::get) panics by name.
    ///
    /// Idempotent: a second call is a no-op.
    ///
    /// # Panics
    ///
    /// Propagates a panic from the suite value's `Drop` — after the runtime has
    /// been shut down regardless.
    #[doc(hidden)]
    pub fn finish<T>(&mut self, value: Option<T>) {
        let Some(runtime) = self.0.take() else {
            return;
        };
        let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if value.is_some() {
                runtime.block_on(async move { drop(value) });
            }
        }));
        runtime.shutdown_timeout(SHUTDOWN_GRACE);
        if let Err(payload) = dropped {
            std::panic::resume_unwind(payload);
        }
    }
}

fn recover<T>(result: Result<T, PoisonError<T>>) -> T {
    result.unwrap_or_else(PoisonError::into_inner)
}

/// Accepted return values for suite hooks and cases.
#[doc(hidden)]
pub trait SuiteOutcome {
    fn assert_passed(self);
}

impl SuiteOutcome for () {
    fn assert_passed(self) {}
}

impl<T, E> SuiteOutcome for Result<T, E>
where
    E: Debug,
{
    fn assert_passed(self) {
        if let Err(err) = self {
            panic!("R2E test suite method returned Err: {err:?}");
        }
    }
}
