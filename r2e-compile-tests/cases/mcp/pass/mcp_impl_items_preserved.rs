//! Non-member items of a `#[routes]` MCP service impl (consts, plain fns)
//! are preserved by the expansion.

#![deny(non_snake_case)]

use r2e::prelude::*;

#[controller]
pub struct PreservedImpl;

#[routes]
#[allow(non_snake_case)]
impl PreservedImpl {
    const LABEL: &'static str = "preserved";

    pub fn Helper() -> &'static str {
        Self::LABEL
    }

    // A member runs on the request façade (like an HTTP route), so an
    // associated item of the controller is named through the controller
    // type, not `Self`.
    #[tool]
    async fn ping(&self) -> String {
        PreservedImpl::Helper().to_string()
    }
}

fn main() {
    assert_eq!(PreservedImpl::Helper(), "preserved");
}
