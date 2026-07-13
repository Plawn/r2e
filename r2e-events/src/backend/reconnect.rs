//! Shared reconnect-with-backoff driver for distributed backend pollers.
//!
//! Every backend's background loops (pollers, responder consumers, reply
//! consumers) carry the same reconnect skeleton: run an inner attempt, and if
//! it returns while the bus is still live and `reconnect` is enabled, back off
//! and try again. [`reconnect_loop`] centralizes that policy so the backends
//! only supply their per-attempt body.

use std::future::Future;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

/// Run `inner` repeatedly with capped exponential backoff between attempts.
///
/// Each iteration awaits one `inner()` attempt (which should return when its
/// connection drops or its stream ends). After it returns:
/// - if `cancel` is cancelled or `reconnect` is `false`, the loop exits;
/// - otherwise it backs off and runs `inner` again.
///
/// Backoff policy (identical across all backends): starts at 1s, doubles each
/// reconnect up to `max_backoff`, and resets to 1s after any attempt that ran
/// healthily for longer than `backoff * 4`. The backoff sleep is cancel-aware —
/// cancelling during the wait exits immediately. `label` names the loop in the
/// reconnect warning (e.g. `"Kafka reply consumer"`); include the topic in it if
/// useful.
pub async fn reconnect_loop<F, Fut>(
    reconnect: bool,
    max_backoff: Duration,
    cancel: &CancellationToken,
    label: &str,
    mut inner: F,
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    let mut backoff = Duration::from_secs(1);

    loop {
        let start = Instant::now();
        inner().await;

        if cancel.is_cancelled() || !reconnect {
            break;
        }

        // Reset backoff after an attempt that stayed healthy for a while.
        if start.elapsed() > backoff * 4 {
            backoff = Duration::from_secs(1);
        }

        tracing::warn!("{label} disconnected, reconnecting in {backoff:?}");
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = r2e_core::rt::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(max_backoff);
    }
}
