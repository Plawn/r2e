//! `RuntimeId`: what it proves, and the one thing it does not.

use std::time::Duration;

use r2e_rt::{Runtime, RuntimeBuilder, RuntimeHandle, RuntimeId};

fn multi_thread() -> Runtime {
    RuntimeBuilder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

#[test]
fn a_handle_carries_the_id_of_its_runtime_across_threads() {
    let runtime = multi_thread();
    let expected = runtime.id();
    let handle = runtime.handle();

    // The point of the type: comparable from a thread that never saw the
    // `Runtime` value, which is how a test suite's guard-rail uses it.
    let observed = std::thread::spawn(move || handle.id())
        .join()
        .expect("thread");
    assert_eq!(observed, expected);
}

#[test]
fn work_running_on_a_runtime_reports_that_runtime() {
    let runtime = multi_thread();
    let expected = runtime.id();

    let from_block_on = runtime.block_on(async { RuntimeHandle::current().id() });
    assert_eq!(from_block_on, expected);

    let from_task = runtime.block_on(async {
        r2e_rt::spawn(async { RuntimeHandle::current().id() })
            .await
            .expect("task")
    });
    assert_eq!(from_task, expected);

    let from_blocking = runtime.block_on(async {
        let handle = RuntimeHandle::current();
        r2e_rt::spawn_blocking(move || handle.id())
            .await
            .expect("blocking task")
    });
    assert_eq!(from_blocking, expected);
}

#[test]
fn two_live_runtimes_have_different_ids() {
    let first = multi_thread();
    let second = multi_thread();
    assert_ne!(first.id(), second.id());
}

#[test]
fn an_id_is_only_meaningful_while_its_runtime_is_alive() {
    // The documented limitation: ids are unique among *live* runtimes, and the
    // one belonging to a dropped runtime may be handed out again. So a cached
    // id is proof of shared identity only against a runtime known to still be
    // running — which is why the suite guard compares against a runtime the
    // suite itself owns. This test does not assert reuse (it is not
    // guaranteed either way), it pins that we never rely on the opposite.
    let stale = {
        let runtime = multi_thread();
        let id = runtime.id();
        runtime.shutdown_timeout(Duration::from_millis(10));
        id
    };
    let live = multi_thread();
    let _: (RuntimeId, RuntimeId) = (stale, live.id());
    // A stale id still prints, so a diagnostic that names it stays useful.
    assert!(!stale.to_string().is_empty());
}

#[test]
fn there_is_no_current_runtime_outside_one() {
    assert!(RuntimeHandle::try_current().is_none());
}
