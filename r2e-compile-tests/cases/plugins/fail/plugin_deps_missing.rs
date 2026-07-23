//! A plugin's `Deps` are appended to the builder's requirement list and
//! verified against the FINAL provision list at `build_state()` (not at the
//! `.plugin()` call site). Here `MissingBean` is declared as a dep but never
//! provided or registered, so `build_state()` must fail with the standard
//! missing-dependency diagnostic.

use r2e::prelude::*;
use r2e::{PluginInstallContext, PreStatePlugin};

#[derive(Clone)]
pub struct MissingBean;

/// A plugin whose dependency is never supplied.
pub struct NeedsBean;

impl PreStatePlugin for NeedsBean {
    type Provided = ();
    type Deps = (MissingBean,);
    type Config = ();

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) {}
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .plugin(NeedsBean)
            // `MissingBean` is never `.provide()`-d or `.register()`-ed.
            .build_state()
            .await
    };
}
