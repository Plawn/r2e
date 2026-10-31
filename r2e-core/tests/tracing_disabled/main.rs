//! `tracing.enabled: false`: `build_state()` installs nothing — the
//! application owns the subscriber, or wants none.
//!
//! Its **own target**: the global subscriber is a one-shot process global.

use r2e_core::config::R2eConfig;
use r2e_core::{installed, AppBuilder};
use tracing::Level;

#[r2e_core::test]
async fn enabled_false_installs_nothing() {
    let config = R2eConfig::from_yaml_str("tracing:\n  enabled: false\n").unwrap();
    let _state = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .build_state()
        .await;

    assert_eq!(installed(), None, "nothing was installed");
    assert!(
        !tracing::enabled!(Level::ERROR),
        "no subscriber: not even ERROR is enabled"
    );
}
