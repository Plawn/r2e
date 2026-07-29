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
//! The hot-patch flag is process-wide and one-way, so it is **also** on for the
//! ordinary tests in this binary once a dev test has marked it. Any test in this
//! target that calls `load_config()` therefore takes this lock for the duration
//! of that call: `load_config` re-seeds the carried registry, which would
//! otherwise land in the middle of a dev test's cycle sequence and clear slots
//! its assertions depend on.

use std::sync::{Mutex, MutexGuard, OnceLock};

pub fn dev_serial() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
