//! `opt_in` hides a group until a session enables it: without `group` there
//! is nothing to enable.

use r2e::prelude::*;

#[controller]
pub struct OptInTools {}

#[mcp_routes]
impl OptInTools {
    /// Broken: `opt_in` on a member without a group.
    #[tool(opt_in)]
    async fn broken(&self) -> String {
        "hi".to_string()
    }
}

fn main() {}
