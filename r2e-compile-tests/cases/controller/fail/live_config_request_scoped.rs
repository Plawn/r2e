//! `#[live_config]` is app-scoped: it cannot be stacked on a request-scoped
//! `#[inject(identity)]` / `#[inject(request)]` controller field.
use r2e::prelude::*;

#[controller(path = "/live")]
pub struct LiveController {
    #[live_config("app.banner")]
    #[inject(identity)]
    banner: LiveConfig<String>,
}

fn main() {}
