//! `#[live_config]` (one live key) vs `#[config_section]` (a boot-time typed
//! section) — the two cannot share a field.
use r2e::prelude::*;

#[derive(Clone, Bean)]
pub struct MyService {
    #[live_config("db.url")]
    #[config_section(prefix = "db")]
    url: LiveConfig<String>,
}

fn main() {}
