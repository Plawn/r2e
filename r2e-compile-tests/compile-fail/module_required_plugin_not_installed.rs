//! A module declaring a required plugin that was NOT installed before it must
//! be rejected at `register_module`, with the diagnostic naming the plugin and
//! pointing at `.plugin(..)` — not surfacing as an opaque missing-bean error on
//! one of the plugin's internal handle types.

use r2e::prelude::*;
use r2e::{PluginInstallContext, PreStatePlugin};

#[derive(Clone)]
pub struct PluginBean;

pub struct MarkerPlugin;

impl PreStatePlugin for MarkerPlugin {
    type Provided = (PluginBean,);
    type Deps = ();
    type LateDeps = ();
    type Config = ();

    fn install(&mut self, (): (), _ctx: &mut PluginInstallContext<'_>) -> (PluginBean,) {
        (PluginBean,)
    }
}

pub struct NeedsPluginModule;

impl FeatureModule for NeedsPluginModule {
    type Providers = r2e::type_list::TNil;
    type Controllers = ();
    type Exports = r2e::type_list::TNil;
    type Imports = r2e::type_list::TNil;
    type RequiredPlugins = (MarkerPlugin,);
}

fn main() {
    // `MarkerPlugin` is never `.plugin(..)`-ed, so its provisions are absent
    // from `P` and the required-plugin check fails.
    let _ = r2e::AppBuilder::new().register_module::<NeedsPluginModule>();
}
