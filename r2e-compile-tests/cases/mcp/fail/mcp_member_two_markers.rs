//! One method, one member family: `#[tool]` + `#[prompt]` on the same method
//! must be rejected.

use r2e::prelude::*;

#[controller]
pub struct DoubleTools {}

#[mcp_routes]
impl DoubleTools {
    /// Broken: two member markers.
    #[tool]
    #[prompt]
    async fn broken(&self) -> String {
        "hi".to_string()
    }
}

fn main() {}
