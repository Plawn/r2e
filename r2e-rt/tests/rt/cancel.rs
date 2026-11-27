//! `CancelToken`, `CancelDropGuard` and `ShutdownToken`.

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use r2e_rt::{CancelToken, ShutdownToken};

/// Poll `future` exactly once with a no-op waker.
fn poll_once<F: Future>(future: F) -> Poll<F::Output> {
    let mut future = pin!(future);
    future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
}

fn same_type_as<T>(_witness: &T, value: T) -> T {
    value
}

#[test]
fn cancel_flips_is_cancelled_and_resolves_cancelled() {
    let token = CancelToken::new();
    assert!(!token.is_cancelled());
    assert!(poll_once(token.cancelled()).is_pending());

    token.cancel();
    assert!(token.is_cancelled());
    assert!(poll_once(token.cancelled()).is_ready());
}

#[test]
fn a_child_token_is_cancelled_with_its_parent_but_not_the_reverse() {
    let parent = CancelToken::new();
    let child = parent.child_token();
    let other_child = parent.child_token();

    other_child.cancel();
    assert!(!parent.is_cancelled());

    parent.cancel();
    assert!(child.is_cancelled());
}

#[test]
fn the_inner_token_is_the_same_signal() {
    let token = CancelToken::new();
    token.inner().cancel();
    assert!(token.is_cancelled());

    let token = CancelToken::new();
    token.clone().into_inner().cancel();
    assert!(token.is_cancelled());
}

#[test]
fn conversions_to_and_from_the_tokio_util_token_keep_the_signal() {
    // `From<CancelToken>` for the raw token: `witness` pins the target type to
    // `into_inner`'s return type without naming the tokio-util crate.
    let token = CancelToken::new();
    let witness = CancelToken::new().into_inner();
    let raw = same_type_as(&witness, token.clone().into());
    raw.cancel();
    assert!(token.is_cancelled());

    // `From<raw>` for `CancelToken`.
    let token = CancelToken::new();
    let back = CancelToken::from(token.clone().into_inner());
    back.cancel();
    assert!(token.is_cancelled());
}

#[test]
fn a_disarmed_guard_returns_the_live_token_without_cancelling_it() {
    let token = CancelToken::new();
    let back = token.clone().drop_guard().disarm();
    assert!(!token.is_cancelled());

    back.cancel();
    assert!(token.is_cancelled());
}

#[test]
fn a_dropped_guard_cancels_the_token() {
    let token = CancelToken::new();
    drop(token.clone().drop_guard());
    assert!(token.is_cancelled());
}

#[test]
fn a_shutdown_token_follows_the_token_it_wraps() {
    let root = CancelToken::new();
    let shutdown = ShutdownToken::from_token(root.clone());
    let child = shutdown.child_token();

    assert!(!shutdown.is_cancelled());
    assert!(poll_once(shutdown.cancelled()).is_pending());
    assert!(!child.is_cancelled());

    root.cancel();
    assert!(shutdown.is_cancelled());
    assert!(poll_once(shutdown.cancelled()).is_ready());
    assert!(child.is_cancelled());
}

#[test]
fn a_shutdown_child_can_be_cancelled_alone() {
    let shutdown = ShutdownToken::detached();
    let child = shutdown.child_token();
    child.cancel();
    assert!(!shutdown.is_cancelled());
}

#[test]
fn the_token_behind_a_shutdown_token_is_the_same_signal() {
    let root = CancelToken::new();
    let shutdown = ShutdownToken::from_token(root.clone());
    shutdown.token().cancel();
    assert!(root.is_cancelled());
    assert!(shutdown.is_cancelled());

    let root = CancelToken::new();
    let unwrapped: CancelToken = ShutdownToken::from_token(root.clone()).into();
    unwrapped.cancel();
    assert!(root.is_cancelled());
}
