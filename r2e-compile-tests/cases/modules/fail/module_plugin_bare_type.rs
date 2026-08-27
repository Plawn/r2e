//! `plugins(...)` needs an instance to install, so a bare plugin type must be
//! rejected with a targeted message pointing at `Type = expr` (and at
//! `requires_plugins(..)` for the "someone else installs it" case).

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

#[module(plugins(MarkerPlugin))]
pub struct BadModule;

fn main() {}
