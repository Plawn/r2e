//! `Runtime` shutdown and `RuntimeHandle` probes.

use std::sync::mpsc;
use std::time::Duration;

use r2e_rt::{Runtime, RuntimeBuilder, RuntimeHandle};

/// Upper bound for waits that only expire when the code under test is broken.
const GENEROUS: Duration = Duration::from_secs(5);

fn multi_thread() -> Runtime {
    RuntimeBuilder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("runtime")
}

/// A runtime whose blocking pool holds a task parked until the returned
/// sender is dropped. Dropping such a runtime blocks; shutting it down
/// explicitly must not.
fn runtime_with_stuck_blocking_task() -> (Runtime, mpsc::Sender<()>) {
    let runtime = multi_thread();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let _job = runtime.block_on(async move {
        r2e_rt::spawn_blocking(move || {
            started_tx.send(()).unwrap();
            let _ = release_rx.recv();
        })
    });
    started_rx.recv().expect("blocking task started");
    (runtime, release_tx)
}

/// Run `shutdown` on another thread and report whether it returned within
/// [`GENEROUS`] while the blocking task is still parked.
fn returns_without_waiting(shutdown: impl FnOnce(Runtime) + Send + 'static) -> bool {
    let (runtime, release) = runtime_with_stuck_blocking_task();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let thread = std::thread::spawn(move || {
        shutdown(runtime);
        let _ = done_tx.send(());
    });
    let returned = done_rx.recv_timeout(GENEROUS).is_ok();
    // Unpark the blocking task so a broken shutdown still lets the thread end.
    drop(release);
    thread.join().expect("shutdown thread");
    returned
}

#[test]
fn shutdown_timeout_does_not_wait_past_its_timeout_for_blocking_work() {
    assert!(returns_without_waiting(|rt| {
        rt.shutdown_timeout(Duration::from_millis(10))
    }));
}

#[test]
fn shutdown_background_does_not_wait_for_blocking_work() {
    assert!(returns_without_waiting(Runtime::shutdown_background));
}

#[test]
fn try_current_is_none_outside_a_runtime_and_names_it_inside() {
    assert!(RuntimeHandle::try_current().is_none());

    let runtime = multi_thread();
    let expected = runtime.id();
    let observed =
        runtime.block_on(async { RuntimeHandle::try_current().expect("inside a runtime").id() });
    assert_eq!(observed, expected);
}

#[test]
fn is_multi_thread_distinguishes_the_two_flavours() {
    assert!(multi_thread().handle().is_multi_thread());

    let current = RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    assert!(!current.handle().is_multi_thread());
}
