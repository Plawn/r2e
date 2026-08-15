//! A pre-state plugin (provides beans, implements `PreStatePlugin`) passed to
//! `.with()` after `build_state()` must be rejected: `.with()` takes a
//! post-state `Plugin`. The diagnostic points the author at `.plugin()` before
//! `build_state()`.

use r2e::prelude::*;
use r2e::{PluginBuildContext, PluginBuildError, PreStatePlugin};

#[derive(Clone)]
pub struct MyBean;

/// A pre-state plugin — provides `MyBean`.
pub struct MyPreStatePlugin;

impl PreStatePlugin for MyPreStatePlugin {
    type Provided = (MyBean,);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        Ok((MyBean,))
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .build_state()
            .await
            // WRONG: pre-state plugin passed to the post-state `.with()`.
            .with(MyPreStatePlugin)
    };
}
