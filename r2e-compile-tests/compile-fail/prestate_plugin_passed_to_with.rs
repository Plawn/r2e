//! A pre-state plugin (provides beans, implements `PreStatePlugin`) passed to
//! `.with()` after `build_state()` must be rejected: `.with()` takes a
//! post-state `Plugin`. The diagnostic points the author at `.plugin()` before
//! `build_state()`.

use r2e::prelude::*;
use r2e::{PluginInstallContext, PreStatePlugin};

#[derive(Clone)]
pub struct MyBean;

/// A pre-state plugin — provides `MyBean`.
pub struct MyPreStatePlugin;

impl PreStatePlugin for MyPreStatePlugin {
    type Provided = (MyBean,);
    type Deps = ();
    type LateDeps = ();
    type Config = ();

    fn install(&mut self, (): (), _ctx: &mut PluginInstallContext<'_>) -> (MyBean,) {
        (MyBean,)
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .build_state()
            .await
            // WRONG: pre-state plugin passed to the post-state `.with()`.
            .with(MyPreStatePlugin)
    };
}
