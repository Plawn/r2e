//! `JobHandle`, `JoinError`, the control plane and the free functions
//! (`yield_now`, `in_runtime`, `lookup_host`, `shutdown_signal`).

use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use r2e_rt::{Runtime, RuntimeBuilder};

/// Upper bound for waits that only expire when the code under test is broken.
const GENEROUS: Duration = Duration::from_secs(5);

fn current_thread() -> Runtime {
    RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// Poll `future` exactly once with a no-op waker.
fn poll_once<F: Future>(future: F) -> Poll<F::Output> {
    let mut future = pin!(future);
    future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn a_panicking_task_reports_a_panic_and_not_a_cancellation() {
    let runtime = current_thread();
    let err = runtime
        .block_on(async { r2e_rt::spawn(async { panic!("boom") }).await })
        .expect_err("the task panicked");
    assert!(err.is_panic());
    assert!(!err.is_cancelled());
}

#[test]
fn an_aborted_task_reports_a_cancellation_and_not_a_panic() {
    let runtime = current_thread();
    let err = runtime.block_on(async {
        let handle = r2e_rt::spawn(std::future::pending::<()>());
        handle.abort();
        r2e_rt::timeout(GENEROUS, handle)
            .await
            .expect("abort resolves the handle")
            .expect_err("the task was aborted")
    });
    assert!(err.is_cancelled());
    assert!(!err.is_panic());
}

#[test]
fn is_finished_tracks_task_completion() {
    let runtime = current_thread();
    runtime.block_on(async {
        // Timer-based yields, not `yield_now`, so a broken `yield_now` cannot
        // turn this loop into a spin the timeout never interrupts.
        let pending = r2e_rt::spawn(std::future::pending::<()>());
        r2e_rt::sleep(Duration::from_millis(1)).await;
        assert!(!pending.is_finished());
        pending.abort();

        let done = r2e_rt::spawn(async {});
        r2e_rt::timeout(GENEROUS, async {
            while !done.is_finished() {
                r2e_rt::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("a ready task finishes");
    });
}

#[test]
fn yield_now_lets_another_ready_task_run() {
    let runtime = current_thread();
    let ran = runtime.block_on(async {
        let flag = Arc::new(AtomicBool::new(false));
        let task_flag = flag.clone();
        let _task = r2e_rt::spawn(async move { task_flag.store(true, Ordering::SeqCst) });
        // Single-threaded runtime: the spawned task can only run if this
        // future actually gives the scheduler back.
        r2e_rt::yield_now().await;
        flag.load(Ordering::SeqCst)
    });
    assert!(ran);
}

#[test]
fn yield_now_is_pending_on_its_first_poll() {
    let runtime = current_thread();
    runtime.block_on(async {
        assert!(poll_once(r2e_rt::yield_now()).is_pending());
    });
}

#[test]
fn in_runtime_reflects_the_calling_context() {
    assert!(!r2e_rt::in_runtime());
    let runtime = current_thread();
    assert!(runtime.block_on(async { r2e_rt::in_runtime() }));
}

#[test]
fn the_control_plane_is_per_thread_and_routes_spawn_ctl() {
    let control = RuntimeBuilder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("control runtime");
    let control_id = control.id();
    let control_handle = control.handle();

    // A fresh thread: no control plane until one is registered.
    std::thread::spawn(move || {
        assert!(r2e_rt::control_plane_handle().is_none());

        r2e_rt::set_control_plane(control_handle);
        let registered = r2e_rt::control_plane_handle().expect("registered");
        assert_eq!(registered.id(), control_id);

        // `spawn_ctl` from a worker runtime lands on the control plane.
        let worker = current_thread();
        let worker_id = worker.id();
        let ran_on = worker.block_on(async {
            r2e_rt::spawn_ctl(async { r2e_rt::RuntimeHandle::current().id() })
                .await
                .expect("task")
        });
        assert_eq!(ran_on, control_id);
        assert_ne!(ran_on, worker_id);
    })
    .join()
    .expect("thread");

    // Registration did not leak onto this thread.
    assert!(r2e_rt::control_plane_handle().is_none());
}

#[test]
fn lookup_host_resolves_a_literal_socket_address() {
    let runtime = current_thread();
    let addrs = runtime
        .block_on(r2e_rt::lookup_host("127.0.0.1:8080"))
        .expect("resolves");
    assert_eq!(addrs, vec!["127.0.0.1:8080".parse().unwrap()]);
}

#[test]
fn shutdown_signal_stays_pending_without_a_signal() {
    let runtime = current_thread();
    runtime.block_on(async {
        // Polled inside the runtime so the signal driver is reachable. No
        // signal is ever sent, so it must not resolve.
        assert!(poll_once(r2e_rt::shutdown_signal()).is_pending());
    });
}
