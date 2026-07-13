//! Tests for the shared `reconnect_loop` backoff driver.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_events::backend::reconnect_loop;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread")]
async fn no_reconnect_runs_inner_exactly_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let cancel = CancellationToken::new();

    let c = calls.clone();
    // reconnect = false → exit after the first attempt, no backoff sleep.
    reconnect_loop(false, Duration::from_secs(30), &cancel, "test", || {
        c.fetch_add(1, Ordering::SeqCst);
        async {}
    })
    .await;

    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn pre_cancelled_token_still_runs_inner_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let cancel = CancellationToken::new();
    cancel.cancel();

    let c = calls.clone();
    // Even reconnect = true exits after one attempt when already cancelled.
    reconnect_loop(true, Duration::from_secs(30), &cancel, "test", || {
        c.fetch_add(1, Ordering::SeqCst);
        async {}
    })
    .await;

    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnects_until_cancelled() {
    let calls = Arc::new(AtomicUsize::new(0));
    let cancel = CancellationToken::new();

    let c = calls.clone();
    // Cancel on the second attempt: the loop reconnects once (one ~1s backoff),
    // then the post-attempt cancel check breaks it — no third attempt.
    reconnect_loop(true, Duration::from_secs(30), &cancel, "test", || {
        let n = c.fetch_add(1, Ordering::SeqCst) + 1;
        if n == 2 {
            cancel.cancel();
        }
        async {}
    })
    .await;

    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
