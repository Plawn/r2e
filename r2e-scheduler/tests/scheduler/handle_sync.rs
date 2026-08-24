use r2e_core::rt::CancelToken;
use r2e_scheduler::SchedulerHandle;

#[test]
fn handle_not_cancelled_initially() {
    let token = CancelToken::new();
    let handle = SchedulerHandle::new(token);
    assert!(!handle.is_cancelled());
}

#[test]
fn handle_cancel_sets_flag() {
    let token = CancelToken::new();
    let handle = SchedulerHandle::new(token);
    handle.cancel();
    assert!(handle.is_cancelled());
}

#[test]
fn handle_token_accessor() {
    let token = CancelToken::new();
    let handle = SchedulerHandle::new(token);
    let retrieved = handle.token();
    retrieved.cancel();
    assert!(handle.is_cancelled());
}

#[test]
fn handle_clone_shares_state() {
    let token = CancelToken::new();
    let handle = SchedulerHandle::new(token);
    let cloned = handle.clone();
    cloned.cancel();
    assert!(handle.is_cancelled());
}

#[test]
fn handle_cancel_idempotent() {
    let token = CancelToken::new();
    let handle = SchedulerHandle::new(token);
    handle.cancel();
    handle.cancel();
    assert!(handle.is_cancelled());
}
