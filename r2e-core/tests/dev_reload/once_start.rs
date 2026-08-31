//! "Once per process" startup work under the hot-patch loop.
//!
//! `AppBuilder::on_start_once` and `#[on_start(once)]` exist for the work a
//! boot is allowed to do exactly once — crash recovery, claiming a lock, a
//! one-off backfill. Under `r2e dev` the app is re-assembled in the same
//! process on every patch, so "once per boot" and "once per process" stop
//! being the same thing, and apps hand-roll a `static BOOTED: AtomicBool` to
//! tell them apart.
//!
//! These cycles are driven **in-process** on purpose. A serving cycle already
//! skips its whole startup lifecycle once `LIFECYCLE_INITIALIZED` is set, so
//! it would show a once-hook not re-running even if the once-guard did not
//! exist. `start_in_process` never takes that skip (`LifecycleMode::InProcess`
//! is excluded from it), so a plain `on_start` here really does run on both
//! cycles — which is exactly what makes the `once` assertion meaningful.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use r2e_core::AppBuilder;

use crate::serial::CommitCycle;

static PLAIN_RUNS: AtomicUsize = AtomicUsize::new(0);
static ONCE_RUNS: AtomicUsize = AtomicUsize::new(0);

type Log = Arc<Mutex<Vec<&'static str>>>;

#[derive(Clone)]
struct RecoveryBean {
    log: Log,
}

#[r2e_macros::bean]
impl RecoveryBean {
    fn new(log: Log) -> Self {
        Self { log }
    }

    #[on_start(once)]
    async fn recover(&self) {
        self.log.lock().unwrap().push("once");
    }

    #[on_start]
    async fn every_cycle(&self) {
        self.log.lock().unwrap().push("always");
    }
}

async fn run_cycle(log: &Log) {
    let app = AppBuilder::new()
        .provide(log.clone())
        .register::<RecoveryBean>()
        .build_state()
        .await
        .commit_cycle()
        .on_start(|_state| async move {
            PLAIN_RUNS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .on_start_once(|_state| async move {
            ONCE_RUNS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

    app.prepare("127.0.0.1:0")
        .start_in_process()
        .await
        .expect("boot")
        .shutdown()
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn once_hooks_do_not_re_run_on_a_hot_patch() {
    let _serial = crate::serial::dev_serial();
    r2e_core::invalidate_state_cache();
    r2e_core::runtime::dev::mark_hot_reload_loop();
    PLAIN_RUNS.store(0, Ordering::SeqCst);
    ONCE_RUNS.store(0, Ordering::SeqCst);

    let log: Log = Arc::new(Mutex::new(Vec::new()));

    // ── Cycle 1: cold start — everything runs ────────────────────────────
    run_cycle(&log).await;
    assert_eq!(PLAIN_RUNS.load(Ordering::SeqCst), 1);
    assert_eq!(ONCE_RUNS.load(Ordering::SeqCst), 1);
    assert_eq!(*log.lock().unwrap(), vec!["once", "always"]);

    // ── Cycle 2: the hot patch ───────────────────────────────────────────
    run_cycle(&log).await;
    assert_eq!(
        PLAIN_RUNS.load(Ordering::SeqCst),
        2,
        "a plain `on_start` closure runs on every in-process cycle — the \
         control that makes the assertion below mean something"
    );
    assert_eq!(
        ONCE_RUNS.load(Ordering::SeqCst),
        1,
        "`on_start_once` must not re-run on a hot patch"
    );
    assert_eq!(
        *log.lock().unwrap(),
        vec!["once", "always", "always"],
        "`#[on_start(once)]` must not re-run either, while its sibling does"
    );
}
