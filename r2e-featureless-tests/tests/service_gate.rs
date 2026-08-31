//! The runtime half of the featureless canary: a `#[service(enabled = "…")]`
//! service registers and validates its config like any other, and simply never
//! reaches `run()`.
//!
//! Everything here is reached through `r2e` with `default-features = false`.

use std::sync::atomic::Ordering;
use std::time::Duration;

use r2e::config::{ConfigValue, R2eConfig};
use r2e::prelude::*;
use r2e_featureless_tests::{GatedWorker, Sink, GATED_WORKER_RAN};

/// A disabled service is still a registered service: its `#[config]` keys are
/// validated exactly as an enabled one's are. Turning a worker off must not
/// turn its configuration errors off with it.
#[tokio::test]
async fn disabled_service_still_validates_its_config() {
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .provide(Sink::default())
        .build_state()
        .await;

    let err = app
        .try_spawn_service::<GatedWorker>()
        .err()
        .expect("the gate must not skip config validation");
    assert!(
        err.to_string().contains("workers.gated.enabled"),
        "the missing key must be named: {err}"
    );
}

/// Boot an app with the gate flag set to `flag`, let the service task have a
/// turn, and report whether `GatedWorker::run` was entered.
async fn ran_with_gate(flag: bool) -> bool {
    GATED_WORKER_RAN.store(false, Ordering::SeqCst);

    let mut config = R2eConfig::empty();
    config.set("workers.gated.enabled", ConfigValue::Bool(flag));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .provide(Sink::default())
        .build_state()
        .await
        .spawn_service::<GatedWorker>();

    let running = app
        .prepare("127.0.0.1:0")
        .start_in_process()
        .await
        .expect("boot");
    r2e::rt::sleep(Duration::from_millis(50)).await;
    running.shutdown().await;

    GATED_WORKER_RAN.load(Ordering::SeqCst)
}

/// One test, both directions: the observation is a single process-global flag,
/// and `cargo test` runs the functions in a test binary concurrently — two
/// tests writing that flag would race each other rather than the gate.
#[tokio::test]
async fn the_gate_decides_whether_run_is_entered() {
    assert!(
        !ran_with_gate(false).await,
        "a service whose `enabled` gate returns false must never enter run()"
    );
    assert!(
        ran_with_gate(true).await,
        "the same service must run normally once its gate returns true"
    );
}
