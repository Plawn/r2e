//! A module may **bring** a plugin: `plugins(Type = expr)` installs it at
//! `register_module`, so the plugin's beans are in the module's local scope
//! (a private provider depends on one here) and another module registered after
//! it can satisfy `requires_plugins(..)` from it.

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

/// Module-private provider depending on the brought plugin's bean.
#[derive(Clone)]
pub struct Svc {
    _bean: PluginBean,
}

#[bean]
impl Svc {
    fn new(bean: PluginBean) -> Self {
        Self { _bean: bean }
    }
}

#[module(providers(Svc), plugins(MarkerPlugin = MarkerPlugin))]
pub struct BringsPluginModule;

#[module(requires_plugins(MarkerPlugin))]
pub struct NeedsPluginModule;

fn main() {
    let _ = r2e::AppBuilder::new()
        .register_module::<BringsPluginModule>()
        .register_module::<NeedsPluginModule>();
}
