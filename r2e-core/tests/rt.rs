use std::time::Duration;
use r2e_core::rt;

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
