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
//! never dropped, so `#[after_all]` still has a live reactor under it. Every
//! hook and every case `block_on`s that same runtime.
//!
//! [`SuiteCell::assert_on_suite_runtime`] is the guard-rail: each case checks,
//! from inside its `block_on`, that it really is on the suite runtime, and
//! panics naming the two runtimes if it is not — rather than letting the suite
//! degrade into a pool timeout that looks like an infrastructure problem.

use std::fmt::Debug;
use std::sync::{Mutex, MutexGuard, PoisonError};

use r2e_core::rt::{Runtime, RuntimeHandle, RuntimeId};

/// Shared state for one generated test suite, plus the runtime every case runs
/// on. See the [module docs](self) for why the runtime lives here.
#[doc(hidden)]
pub struct SuiteCell<T> {
    total_cases: usize,
    runtime: Runtime,
    runtime_id: RuntimeId,
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

impl<T> SuiteCell<T> {
    /// Create a suite cell for `total_cases` generated `#[case]` tests, taking
    /// ownership of the runtime the whole suite will run on.
    #[doc(hidden)]
    pub fn new(total_cases: usize, runtime: Runtime) -> Self {
        let runtime_id = runtime.id();
        Self {
            total_cases,
            runtime,
            runtime_id,
            state: Mutex::new(SuiteState {
                suite: None,
                init_failed: false,
                completed_cases: 0,
                after_all_ran: false,
            }),
        }
    }

    /// The suite runtime — `#[before_all]`, `#[before_each]`, every `#[case]`,
    /// `#[after_each]` and `#[after_all]` are driven by this one.
    #[doc(hidden)]
    pub fn runtime(&self) -> &Runtime {
        &self.runtime
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

    /// Guard-rail: assert the caller is running on the suite runtime.
    ///
    /// Called from inside each case's `block_on`. It cannot fail as long as the
    /// generated code goes through [`runtime`](Self::runtime) — which is the
    /// point: if that ever regresses, the suite fails here, naming the cause,
    /// instead of hanging on a resource whose reactor is gone.
    ///
    /// # Panics
    ///
    /// If the calling thread is not driven by the suite runtime.
    #[doc(hidden)]
    pub fn assert_on_suite_runtime(&self, suite: &str, case: &str) {
        let expected = self.runtime_id;
        match RuntimeHandle::try_current() {
            Some(handle) if handle.id() == expected => {}
            Some(handle) => panic!(
                "R2E test suite `{suite}`: case `{case}` is running on runtime {} but the suite \
                 state was built on runtime {expected}. Everything the suite holds that is bound \
                 to a reactor (a TestApp, a database pool, spawned tasks, timers) is registered \
                 with the suite runtime and would silently stop waking here — typically surfacing \
                 as a timeout, not as an error. Every case must run on the suite runtime \
                 (`SuiteCell::runtime`); this is a `#[r2e::test_suite]` bug, please report it.",
                handle.id()
            ),
            None => panic!(
                "R2E test suite `{suite}`: case `{case}` is not running on any runtime, so the \
                 suite's runtime-bound resources (a TestApp, a database pool, spawned tasks, \
                 timers) cannot make progress. Every case must run on the suite runtime \
                 (`SuiteCell::runtime`, id {expected}); this is a `#[r2e::test_suite]` bug, \
                 please report it."
            ),
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
