//! A bare *expression* in `plugins(...)` is equally invalid: the plugin type is
//! needed at compile time, so the `Type = expr` form is mandatory.

use r2e::prelude::*;
use r2e::{Plugin, PluginBuildContext, PluginBuildError};

#[derive(Clone)]
pub struct PluginBean;

pub struct MarkerPlugin;

impl MarkerPlugin {
    fn new() -> Self {
        Self
    }
}

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

#[module(plugins(MarkerPlugin::new()))]
pub struct BadModule;

fn main() {}
