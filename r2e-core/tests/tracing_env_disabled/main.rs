//! `R2E_TRACING_ENABLED=false` reaches `tracing.enabled` through the env
//! overlay: `build_state()` installs nothing.
//!
//! Its **own target**: the global subscriber is a one-shot process global, and
//! the variable is process-wide. The only test in the binary, so it sets the
//! variable directly (no `env_lock()` needed: nothing else reads the env here).

use r2e_core::{installed, AppBuilder};
use tracing::Level;

#[r2e_core::test]
async fn env_overlay_disables_the_install() {
    // SAFETY: the only test of this binary; no other thread reads the env.
    unsafe { std::env::set_var("R2E_TRACING_ENABLED", "false") };

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("application.yaml"), "app:\n  name: env\n").unwrap();
    let _state = AppBuilder::new()
        .with_config_file(dir.path().join("application.yaml"))
        .load_config::<()>()
        .build_state()
        .await;

    unsafe { std::env::remove_var("R2E_TRACING_ENABLED") };
    assert_eq!(installed(), None, "nothing was installed");
    assert!(!tracing::enabled!(Level::ERROR));
}
