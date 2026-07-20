use r2e_core::rt;
use std::time::Duration;

#[tokio::test]
async fn spawn_and_await_result() {
    let handle = rt::spawn(async { 42u32 });
    let result = handle.await.expect("task should not fail");
    assert_eq!(result, 42);
}

#[tokio::test]
async fn job_handle_abort_is_cancelled() {
    let handle = rt::spawn(async {
        rt::sleep(Duration::from_secs(60)).await;
    });
    handle.abort();
    let err = handle.await.expect_err("aborted task should return Err");
    assert!(err.is_cancelled(), "expected cancelled, got: {err}");
    assert!(!err.is_panic());
}

#[tokio::test]
async fn job_handle_is_finished() {
    let handle = rt::spawn(async { 1u8 });
    let _ = handle.await;
    // After awaiting, the task is finished — but the handle is consumed.
    // Verify is_finished on a fresh short-lived task before awaiting.
    let handle2 = rt::spawn(async {});
    // Yield to let the task complete.
    tokio::task::yield_now().await;
    assert!(handle2.is_finished());
    let _ = handle2.await;
}

#[tokio::test]
async fn timeout_success() {
    let result = rt::timeout(Duration::from_millis(100), async { "ok" }).await;
    assert_eq!(result.expect("should not time out"), "ok");
}

#[tokio::test]
async fn timeout_expiry() {
    let result = rt::timeout(
        Duration::from_millis(10),
        rt::sleep(Duration::from_secs(60)),
    )
    .await;
    let err = result.expect_err("should have timed out");
    assert!(err.to_string().contains("elapsed") || !err.to_string().is_empty());
}

#[tokio::test]
async fn bind_tcp_port_zero() {
    let listener = rt::bind_tcp("127.0.0.1:0")
        .await
        .expect("bind on port 0 should succeed");
    let addr = listener.local_addr().expect("should have local addr");
    assert!(addr.port() > 0, "OS should assign a non-zero port");
}

#[tokio::test]
async fn join_error_is_panic_on_panicking_task() {
    let handle = rt::spawn(async { panic!("intentional panic in test") });
    let err = handle.await.expect_err("panicking task should return Err");
    assert!(err.is_panic(), "expected panic, got: {err}");
    assert!(!err.is_cancelled());
}

#[tokio::test]
async fn spawn_ctl_without_control_plane_behaves_like_spawn() {
    // No control-plane handle registered on this thread → spawn_ctl is a plain
    // spawn onto the current runtime.
    let handle = rt::spawn_ctl(async { 7u32 });
    let result = handle.await.expect("task should not fail");
    assert_eq!(result, 7);
}

#[test]
fn spawn_ctl_with_control_plane_runs_on_control_plane_runtime() {
    use tokio::runtime::RuntimeFlavor;

    // The control plane is a multi-thread runtime.
    let control_plane = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("build control-plane runtime");
    let cp_handle = control_plane.handle().clone();

    // Simulate a sharded worker thread: a current_thread runtime on a thread
    // where the control-plane handle is registered.
    let worker = std::thread::Builder::new()
        .name("test-worker".to_string())
        .spawn(move || {
            rt::set_control_plane(cp_handle);
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build worker runtime");
            rt.block_on(async {
                // From inside the worker's current_thread runtime, spawn_ctl
                // must land on the multi-thread control plane.
                let flavor =
                    rt::spawn_ctl(async { tokio::runtime::Handle::current().runtime_flavor() })
                        .await
                        .expect("ctl task should not fail");
                assert_eq!(
                    flavor,
                    RuntimeFlavor::MultiThread,
                    "spawn_ctl must execute on the multi-thread control plane"
                );

                // A plain spawn, by contrast, stays on the worker's current_thread runtime.
                let local_flavor =
                    rt::spawn(async { tokio::runtime::Handle::current().runtime_flavor() })
                        .await
                        .expect("local task should not fail");
                assert_eq!(local_flavor, RuntimeFlavor::CurrentThread);
            });
        })
        .expect("spawn worker thread");

    worker.join().expect("worker thread should not panic");
}
