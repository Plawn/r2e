//! A plugin's `Controllers` are registered by the framework at `build_state()`
//! and dependency-checked against the FINAL provision list — the plugin's own
//! `Provided` tuple included. Here the controller injects a bean nobody
//! supplies, so `build_state()` must fail, and the diagnostic must name the
//! plugin that pulled the controller in.

use r2e::prelude::*;
use r2e::{Plugin, PluginBuildContext, PluginBuildError};

#[derive(Clone)]
pub struct PluginBean;

/// Never provided by anyone.
#[derive(Clone)]
pub struct MissingBean;

#[controller(path = "/shipped")]
pub struct ShippedController {
    #[inject]
    provided: PluginBean,
    #[inject]
    missing: MissingBean,
}

#[routes]
impl ShippedController {
    #[get("/")]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

/// Ships a controller whose second dependency is unsatisfiable.
pub struct ShipsController;

impl Plugin for ShipsController {
    type Provided = (PluginBean,);
    type Deps = ();
    type Config = ();
    type Controllers = (ShippedController,);

    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        Ok((PluginBean,))
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .plugin(ShipsController)
            // `MissingBean` is never `.provide()`-d or `.register()`-ed.
            .build_state()
            .await
    };
}
