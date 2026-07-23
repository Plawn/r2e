//! A post-state plugin (implements `Plugin`) passed to `.plugin()` before
//! `build_state()` must be rejected: `.plugin()` takes a pre-state plugin. The
//! diagnostic points the author at `.with()` after `build_state()`.

use r2e::prelude::*;
use r2e::Plugin;

/// A post-state plugin — installs after `build_state()`.
pub struct MyPostStatePlugin;

impl Plugin for MyPostStatePlugin {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            // WRONG: post-state plugin passed to the pre-state `.plugin()`.
            .plugin(MyPostStatePlugin)
            .build_state()
            .await
    };
}
