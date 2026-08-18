//! `SchedulerHandle` runtime-control edge cases and the `FromRequestParts`
//! extractor, plus `ScheduledJobRegistry::default`.
//!
//! These cover the branches that don't require a running driver:
//! - a handle with no command channel ([`SchedulerHandle::new`]) → control is a no-op;
//! - a wired handle whose driver receiver was dropped → send fails gracefully;
//! - extracting the handle from request extensions (present and absent).

use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::header::HttpRequest;
use r2e_scheduler::{ScheduledJobInfo, ScheduledJobRegistry, SchedulerHandle};
use tokio_util::sync::CancellationToken;

// ── SchedulerHandle::send edge cases ────────────────────────────────────────

#[tokio::test]
async fn handle_without_commands_is_a_noop() {
    // Built via `new` — no command channel, so every control call returns false
    // (the `self.commands` is None short-circuit).
    let handle = SchedulerHandle::new(CancellationToken::new());
    assert!(!handle.pause("anything").await, "pause is a no-op");
    assert!(!handle.resume("anything").await, "resume is a no-op");
    assert!(!handle.trigger_now("anything").await, "trigger is a no-op");
}

#[tokio::test]
async fn handle_returns_false_when_driver_receiver_is_gone() {
    // A wired handle whose `SchedulerCommands` receiver was dropped: the send
    // fails and the control call reports false instead of hanging.
    let (handle, commands) = SchedulerHandle::channel(CancellationToken::new());
    drop(commands); // no driver ever reads the channel

    assert!(!handle.pause("job").await, "send fails → false");
    assert!(!handle.resume("job").await);
    assert!(!handle.trigger_now("job").await);
}

#[tokio::test]
async fn handle_cancellation_helpers() {
    let token = CancellationToken::new();
    let handle = SchedulerHandle::new(token.clone());
    assert!(!handle.is_cancelled());
    handle.cancel();
    assert!(handle.is_cancelled());
    assert!(handle.token().is_cancelled());
}

// ── FromRequestParts ────────────────────────────────────────────────────────

#[tokio::test]
async fn extractor_succeeds_when_handle_is_in_extensions() {
    let (mut parts, _) = HttpRequest::builder().body(()).unwrap().into_parts();
    parts
        .extensions
        .insert(SchedulerHandle::new(CancellationToken::new()));

    let extracted = SchedulerHandle::from_request_parts(&mut parts, &())
        .await
        .expect("handle present in extensions");
    assert!(!extracted.is_cancelled());
}

#[tokio::test]
async fn extractor_errors_when_scheduler_not_installed() {
    let (mut parts, _) = HttpRequest::builder().body(()).unwrap().into_parts();

    let err = SchedulerHandle::from_request_parts(&mut parts, &())
        .await
        .err()
        .expect("no handle in extensions → error");
    assert_eq!(err.0, r2e_core::http::StatusCode::INTERNAL_SERVER_ERROR);
    assert!(err.1.contains("Scheduler not installed"));
}

// ── ScheduledJobRegistry::default ───────────────────────────────────────────

#[test]
fn registry_default_is_empty() {
    let reg = ScheduledJobRegistry::default();
    assert!(reg.list_jobs().is_empty());
    // Sanity: it is a working registry.
    reg.register(ScheduledJobInfo::new("x", "every 1s"));
    assert_eq!(reg.list_jobs().len(), 1);
}

#[test]
fn update_job_is_a_noop_when_the_name_is_absent() {
    let reg = ScheduledJobRegistry::default();
    reg.register(ScheduledJobInfo::new("known", "every 1s"));

    // Mutating a present entry applies; the closure runs.
    reg.update_job("known", |info| info.run_count += 1);
    assert_eq!(reg.job("known").unwrap().run_count, 1);

    // Mutating an absent entry is a no-op: the closure must never run, and
    // the registry is left untouched.
    reg.update_job("ghost", |_| {
        panic!("closure must not run for an absent job")
    });
    assert_eq!(reg.list_jobs().len(), 1);
    assert!(reg.job("ghost").is_none());
}
