//! A foreign global subscriber installed before the builder wins: R2E's
//! install loses, the contributed layers are ignored (one WARN through the
//! foreign subscriber), and `build_state()` still succeeds.
//!
//! Its **own target**: the global subscriber is a one-shot process global.

use r2e_core::{installed, AppBuilder, Suppress};

#[r2e_core::test]
async fn a_foreign_subscriber_wins_and_the_build_succeeds() {
    tracing::subscriber::set_global_default(tracing_subscriber::fmt().finish())
        .expect("the foreign subscriber is first");

    let _state = AppBuilder::new()
        .tracing_layer(Suppress::error("t", "m"))
        .build_state()
        .await;

    assert_eq!(installed(), None, "R2E's install lost to the foreign one");
}
