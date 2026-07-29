//! A bare `#[live_config]` (no key) must name the missing argument instead of
//! failing with syn's generic "expected attribute arguments" message.
use r2e::prelude::*;

#[derive(Clone, Bean)]
pub struct MyService {
    #[live_config]
    url: LiveConfig<String>,
}

fn main() {}
