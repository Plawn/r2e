//! `AppBuilder::on_start_once` and `#[on_start(once)]` outside the hot-patch
//! loop.
//!
//! Outside `r2e dev` there is exactly one cycle per application, so `once`
//! must be indistinguishable from a plain `on_start`: it runs at boot, it runs
//! in the same slot, and it runs for *every* application this process builds
//! (a test binary boots dozens — suppressing all but the first would be a
//! spectacular way to break test isolation). The suppression is the hot-patch
//! case, and it is asserted in the `dev_reload` target, which owns the
//! process-global loop flag.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use r2e_core::AppBuilder;

static ONCE_HOOK_RUNS: AtomicUsize = AtomicUsize::new(0);

type Log = Arc<Mutex<Vec<&'static str>>>;

#[derive(Clone)]
struct OnceBean {
    log: Log,
}

#[r2e_macros::bean]
impl OnceBean {
    fn new(log: Log) -> Self {
        Self { log }
    }

    /// `once` composes with `order`: the hook keeps its place in the global
    /// ordering whether or not it ends up running.
    #[on_start(once, order = -5)]
    async fn recover(&self) {
        self.log.lock().unwrap().push("once");
    }

    #[on_start]
    fn every_cycle(&self) {
        self.log.lock().unwrap().push("always");
    }
}

#[tokio::test]
async fn on_start_once_runs_on_a_normal_boot() {
    ONCE_HOOK_RUNS.store(0, Ordering::SeqCst);

    for _ in 0..2 {
        let app = AppBuilder::new()
            .build_state()
            .await
            .on_start_once(|_state| async move {
                ONCE_HOOK_RUNS.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });

        app.prepare("127.0.0.1:0")
            .start_in_process()
            .await
            .expect("boot")
            .shutdown()
            .await;
    }

    // Two applications, two boots, two runs. "Once per process" is scoped to
    // the hot-patch loop; a plain boot has nothing to deduplicate against.
    assert_eq!(
        ONCE_HOOK_RUNS.load(Ordering::SeqCst),
        2,
        "outside `r2e dev`, every application runs its own once-hooks"
    );
}

#[tokio::test]
async fn on_start_once_attribute_keeps_its_place_in_the_order() {
    let log: Log = Arc::new(Mutex::new(Vec::new()));

    let _router = AppBuilder::new()
        .provide(log.clone())
        .register::<OnceBean>()
        .build_state()
        .await
        .build_with_consumers()
        .await;

    assert_eq!(
        *log.lock().unwrap(),
        vec!["once", "always"],
        "`#[on_start(once, order = -5)]` must still sort by its order"
    );
}
