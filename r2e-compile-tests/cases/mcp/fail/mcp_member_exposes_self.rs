//! A member runs on the request façade, where `Self` is not the controller:
//! exposing `Self` in its signature is rejected like an HTTP route does.

use r2e::prelude::*;

#[controller]
pub struct SelfTools;

#[mcp_routes]
impl SelfTools {
    #[tool]
    async fn clone_me(&self) -> Self {
        SelfTools
    }
}

fn main() {}
