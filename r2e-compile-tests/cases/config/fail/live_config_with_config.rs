//! `#[live_config]` and `#[config]` are two different scopes for one slot:
//! stacking them on the same field is a compile error.
use r2e::prelude::*;

#[derive(Clone, Bean)]
pub struct MyService {
    #[live_config("db.url")]
    #[config("db.url")]
    url: LiveConfig<String>,
}

fn main() {}
