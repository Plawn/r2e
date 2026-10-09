//! Where the pool drains in the shutdown sequence (#1071).
//!
//! The executor's drain used to run at step 2 (plugin async hooks), while the
//! listener was still serving: once the pool had begun draining every `submit`
//! came back `RejectedError::Shutdown`, so an in-flight handler — or a sink
//! fed by one — could not hand its work to the pool. The drain now runs at
//! step 5, after the HTTP drain and after the `AfterDrain` services have been
//! stopped, so a sink that flushes through the pool on cancellation is served.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use r2e_core::rt::CancelToken;
use r2e_core::type_list::{TCons, TNil};
use r2e_core::{AppBuilder, BeanContext, ServiceComponent, SpawnService, StopPhase};
use r2e_executor::{Executor, PoolExecutor};

static FLUSHED: AtomicBool = AtomicBool::new(false);
static FLUSH_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// An after-drain sink whose last act is to push a flush job through the pool
/// and wait for it. Before #1071 that `submit` was refused: the pool was
/// already draining by the time anything after step 2 ran.
struct FlushThroughPool {
    pool: PoolExecutor,
}

impl ServiceComponent for FlushThroughPool {
    type Deps = TCons<PoolExecutor, TNil>;

    fn from_context(ctx: &BeanContext) -> Self {
        FlushThroughPool {
            pool: ctx.get::<PoolExecutor>(),
        }
    }

    fn stop_phase() -> StopPhase {
        StopPhase::AfterDrain
    }

    async fn start(self, shutdown: CancelToken) {
        shutdown.cancelled().await;
        match self.pool.submit_named("final-flush", async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            FLUSHED.store(true, Ordering::SeqCst);
        }) {
            Ok(handle) => {
                if let Err(e) = handle.await {
                    *FLUSH_ERROR.lock().unwrap() = Some(format!("flush job failed: {e:?}"));
                }
            }
            Err(rejected) => {
                *FLUSH_ERROR.lock().unwrap() = Some(format!("submit rejected: {rejected:?}"));
            }
        }
    }
}

#[tokio::test]
async fn an_after_drain_sink_can_still_submit_to_the_pool_when_told_to_stop() {
    let app = AppBuilder::new()
        .plugin(Executor)
        .build_state()
        .await
        .spawn_service::<FlushThroughPool>()
        .prepare("127.0.0.1:0")
        .start_in_process()
        .await
        .expect("boot");
    tokio::time::sleep(Duration::from_millis(50)).await;

    tokio::time::timeout(Duration::from_secs(10), app.shutdown())
        .await
        .expect("shutdown must complete");

    assert_eq!(
        FLUSH_ERROR.lock().unwrap().take(),
        None,
        "the pool must accept a submit from an after-drain service: it drains only after \
         those services have been stopped"
    );
    assert!(
        FLUSHED.load(Ordering::SeqCst),
        "the flush job must have run to completion before the pool drained"
    );
}
