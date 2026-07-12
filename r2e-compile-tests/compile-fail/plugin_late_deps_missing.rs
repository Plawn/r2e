//! A plugin's `LateDeps` are appended to the builder's requirement list and
//! verified against the FINAL provision list at `build_state()` (not at the
//! `.plugin()` call site). Here `MissingBean` is declared as a `LateDeps` but
//! never provided or registered, so `build_state()` must fail with the standard
//! missing-dependency diagnostic.

use r2e::prelude::*;
use r2e::{PluginInstallContext, PreStatePlugin};

#[derive(Clone)]
pub struct MissingBean;

/// A plugin whose post-state dependency is never supplied.
pub struct NeedsLateBean;

impl PreStatePlugin for NeedsLateBean {
    type Provided = ();
    type Deps = ();
    type LateDeps = (MissingBean,);
    type Config = ();

    fn install(&mut self, (): (), _ctx: &mut PluginInstallContext<'_>) {}
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .plugin(NeedsLateBean)
            // `MissingBean` is never `.provide()`-d or `.register()`-ed.
            .build_state()
            .await
    };
}
