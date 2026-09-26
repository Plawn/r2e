//! `notifications/progress`: the [`Progress`] reporter a member takes as a
//! parameter to tell the client how far a long call has got.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rmcp::model::{ProgressNotificationParam, ProgressToken};
use rmcp::service::{Peer, RoleServer};

/// Reports progress of the current call to the client
/// (`notifications/progress`).
///
/// Take it as a member parameter (`progress: Progress`) on a tool, resource
/// or prompt — also on [`ToolCall::progress`](crate::ToolCall::progress) and
/// its resource/prompt counterparts.
///
/// Progress is advisory:
/// - when the client sent no `progressToken` with the request, every
///   [`report`](Self::report) is a no-op (the spec forbids unsolicited
///   progress) — [`is_requested`](Self::is_requested) tells which;
/// - the spec requires `progress` to strictly increase: a report that does
///   not is dropped (logged at `debug`) instead of sending invalid wire;
/// - delivery failures are logged at `debug`, never surfaced.
///
/// Reports ride the SSE stream of the request's own POST (rmcp switches a
/// `mcp.json-response` reply to SSE when a notification precedes it). Once
/// the member returns, that stream closes and later reports — e.g. from a
/// task the member spawned — are dropped.
#[derive(Clone)]
pub struct Progress {
    inner: Option<Arc<Reporter>>,
}

struct Reporter {
    token: ProgressToken,
    peer: Peer<RoleServer>,
    /// `f64` bits of the last value sent; `NO_REPORT` before the first one.
    last: AtomicU64,
}

/// Sentinel for "nothing reported yet" — the bits of `-inf`, below every
/// finite progress value.
const NO_REPORT: u64 = 0xFFF0_0000_0000_0000;

impl Progress {
    /// A reporter that never sends anything — what hand-built calls carry.
    pub fn disabled() -> Self {
        Progress { inner: None }
    }

    /// A reporter bound to the request's `progressToken`, if it sent one.
    pub(crate) fn new(token: Option<ProgressToken>, peer: &Peer<RoleServer>) -> Self {
        Progress {
            inner: token.map(|token| {
                Arc::new(Reporter {
                    token,
                    peer: peer.clone(),
                    last: AtomicU64::new(NO_REPORT),
                })
            }),
        }
    }

    /// Whether the client asked for progress (sent a `progressToken`).
    pub fn is_requested(&self) -> bool {
        self.inner.is_some()
    }

    /// Send `notifications/progress`: `progress` so far, out of `total` when
    /// known, with an optional human-readable `message`.
    ///
    /// A no-op without a `progressToken`; a non-increasing (or non-finite)
    /// `progress` is dropped.
    pub async fn report(&self, progress: f64, total: Option<f64>, message: Option<&str>) {
        let Some(reporter) = &self.inner else {
            return;
        };
        if !progress.is_finite() || !reporter.advance(progress) {
            tracing::debug!(
                progress,
                "MCP progress report dropped: progress must strictly increase"
            );
            return;
        }
        let mut param = ProgressNotificationParam::new(reporter.token.clone(), progress);
        param.total = total;
        param.message = message.map(str::to_owned);
        if let Err(err) = reporter.peer.notify_progress(param).await {
            tracing::debug!(error = %err, "MCP progress notification not delivered");
        }
    }
}

impl Reporter {
    /// Record `progress` as the last value if it is above the current one.
    fn advance(&self, progress: f64) -> bool {
        let mut current = self.last.load(Ordering::Acquire);
        loop {
            if progress <= f64::from_bits(current) {
                return false;
            }
            match self.last.compare_exchange_weak(
                current,
                progress.to_bits(),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }
}

impl std::fmt::Debug for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Progress")
            .field("requested", &self.is_requested())
            .finish()
    }
}
