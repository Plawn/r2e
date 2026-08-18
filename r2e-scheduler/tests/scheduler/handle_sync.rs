use r2e_scheduler::SchedulerHandle;
use tokio_util::sync::CancellationToken;

#[test]
fn handle_not_cancelled_initially() {
    let token = CancellationToken::new();
    let handle = SchedulerHandle::new(token);
    assert!(!handle.is_cancelled());
}

#[test]
fn handle_cancel_sets_flag() {
    let token = CancellationToken::new();
    let handle = SchedulerHandle::new(token);
    handle.cancel();
    assert!(handle.is_cancelled());
}

#[test]
fn handle_token_accessor() {
    let token = CancellationToken::new();
    let handle = SchedulerHandle::new(token);
    let retrieved = handle.token();
    retrieved.cancel();
    assert!(handle.is_cancelled());
}

#[test]
fn handle_clone_shares_state() {
    let token = CancellationToken::new();
    let handle = SchedulerHandle::new(token);
    let cloned = handle.clone();
    cloned.cancel();
    assert!(handle.is_cancelled());
}

#[test]
fn handle_cancel_idempotent() {
    let token = CancellationToken::new();
    let handle = SchedulerHandle::new(token);
    handle.cancel();
    handle.cancel();
    assert!(handle.is_cancelled());
}
