//! A member takes at most one `Progress` reporter — two would race on the
//! same `progressToken`'s monotonic counter.

use r2e::prelude::*;

#[controller]
pub struct TwoProgress {}

#[mcp_routes]
impl TwoProgress {
    /// Broken.
    #[tool]
    async fn broken(&self, a: Progress, b: Progress) -> String {
        let _ = (a, b);
        String::new()
    }
}

fn main() {}
