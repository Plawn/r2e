//! Non-regression: `AppBuilder::with(..)` — the post-state plugin install of
//! the PRE-820a5a8 API — was removed. A leftover call must fail at compile
//! time with the migration hint, not with a bare "no method named `with`".

use r2e::prelude::*;
use r2e::{Plugin, PluginBuildContext, PluginBuildError};

pub struct MyPlugin;

impl Plugin for MyPlugin {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        Ok(())
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .build_state()
            .await
            // Old API: plugins used to install AFTER `build_state()`.
            .with(MyPlugin)
    };
}
