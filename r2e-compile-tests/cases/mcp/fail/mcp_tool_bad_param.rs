//! A `#[tool]` method with an unsupported parameter — beans/config go on the
//! struct, tool arguments go through `Params<T>`; a bare parameter must be
//! rejected with a targeted error naming the supported forms.

use r2e::prelude::*;

#[controller]
pub struct BadTools {}

#[mcp_routes]
impl BadTools {
    /// Broken.
    #[tool]
    async fn broken(&self, name: String) -> String {
        name
    }
}

fn main() {}
