//! Runtime support for `#[r2e::test_suite]`.

use std::fmt::Debug;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Shared state for one generated test suite.
#[doc(hidden)]
pub struct SuiteCell<T> {
    total_cases: usize,
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
    /// Create a suite cell for `total_cases` generated `#[case]` tests.
    #[doc(hidden)]
    pub fn new(total_cases: usize) -> Self {
        Self {
            total_cases,
            state: Mutex::new(SuiteState {
                suite: None,
                init_failed: false,
                completed_cases: 0,
                after_all_ran: false,
            }),
        }
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
