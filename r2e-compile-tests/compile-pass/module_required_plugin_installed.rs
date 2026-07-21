//! A module declaring a required plugin compiles when that plugin is installed
//! (via `.plugin(..)`) before `register_module` — its provisions are in `P`.

use r2e::prelude::*;
use r2e::{PluginInstallContext, PreStatePlugin};

#[derive(Clone)]
pub struct PluginBean;

pub struct MarkerPlugin;

impl PreStatePlugin for MarkerPlugin {
    type Provided = (PluginBean,);
    type Deps = ();
    type Config = ();

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) -> (PluginBean,) {
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
    let _ = r2e::AppBuilder::new()
        .plugin(MarkerPlugin)
        .register_module::<NeedsPluginModule>();
}
