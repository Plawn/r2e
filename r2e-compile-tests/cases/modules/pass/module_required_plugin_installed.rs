//! A module declaring a required plugin compiles when that plugin is installed
//! (via `.plugin(..)`) before `register_module` — its provisions are in `P`.

use r2e::prelude::*;
use r2e::{Plugin, PluginBuildContext, PluginBuildError};

#[derive(Clone)]
pub struct PluginBean;

pub struct MarkerPlugin;

impl Plugin for MarkerPlugin {
    type Provided = (PluginBean,);
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        Ok((PluginBean,))
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
