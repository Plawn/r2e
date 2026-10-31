//! The `Tracing` plugin was removed: the subscriber is installed by
//! `build_state()` from the `tracing:` section. `.plugin(Tracing)` fails
//! naming the migration page.

use r2e::prelude::*;

async fn boot() {
    let _ = AppBuilder::new().plugin(Tracing).build_state().await;
}

fn main() {
    let _ = boot;
}
