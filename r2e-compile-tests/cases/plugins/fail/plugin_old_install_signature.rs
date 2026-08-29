//! Non-regression: a plugin still written against the PRE-820a5a8 API.
//!
//! The old post-state trait had a single `install(self, app) -> AppBuilder<T>`
//! method; the one surviving `Plugin` trait takes `type Provided`/`Deps`/
//! `Config`/`Controllers` plus an async `build(self, deps, config, ctx)`.
//! Compiling the old shape must point at `docs/migration/plugin-api.md`.

use r2e::prelude::*;
use r2e::Plugin;

pub struct Legacy;

impl Plugin for Legacy {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app
    }
}

fn main() {
    let _ = async {
        AppBuilder::new().plugin(Legacy).build_state().await
    };
}
