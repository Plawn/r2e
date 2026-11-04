//! `#[routes(opt_in)]` needs the service-wide `group` it hides.

use r2e::prelude::*;

#[controller]
pub struct OptInService {}

#[routes(opt_in)]
impl OptInService {
    #[tool]
    async fn ping(&self) -> String {
        "pong".to_string()
    }
}

fn main() {}
