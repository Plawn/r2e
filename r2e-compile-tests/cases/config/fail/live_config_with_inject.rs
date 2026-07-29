//! `#[live_config]` resolves the handle itself — it cannot be combined with
//! `#[inject]` (which clones a bean of the field type).
use r2e::prelude::*;

#[derive(Clone, Bean)]
pub struct MyService {
    #[live_config("db.url")]
    #[inject]
    url: LiveConfig<String>,
}

fn main() {}
