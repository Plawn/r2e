//! Serialization for everything that touches the process-global dev-reload
//! state.
//!
//! The dev-reload caches (`STATE_CACHE`, `CTX_CACHE`, `GRAPH_FINGERPRINT`,
//! `PER_BEAN_FINGERPRINTS`, `LIFECYCLE_INITIALIZED`, and the carried
//! `LiveConfigRegistry`) are process-global, so two test functions driving
//! `build_state()` inside the hot-patch loop would clobber each other's cycles.
//! Every such test owns this lock for its whole body and starts from a cold
//! cache (`invalidate_state_cache()`).
//!
//! This lock covers the tests **inside this target only**. It cannot protect
//! anything outside it: the hot-patch flag is process-wide and one-way, which
//! is why the dev-reload tests get their own test binary (see `main.rs`)
//! instead of sharing the `runtime` one.

use std::sync::{Mutex, MutexGuard, OnceLock};

pub fn dev_serial() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The commit step of the hot-patch loop.
///
/// `try_build_state()` **stages** a cycle rather than publishing it: only
/// `r2e::launch!` knows whether the rest of the assembly (`App::build`,
/// deferred controllers) succeeded, so it calls `commit_dev_cycle()` on `Ok`
/// and `rollback_dev_cycle()` on `Err`. Tests that drive cycles by hand play
/// the loop's part, so they must commit too — otherwise the next cycle finds
/// an empty cache and rebuilds cold.
pub trait CommitCycle: Sized {
    fn commit_cycle(self) -> Self {
        r2e_core::commit_dev_cycle();
        self
    }
}

impl<T> CommitCycle for T {}
