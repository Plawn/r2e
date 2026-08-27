#![deny(non_snake_case)]

use r2e::prelude::*;

#[controller]
pub struct PreservedImpl;

#[mcp_routes]
#[allow(non_snake_case)]
impl PreservedImpl {
    const LABEL: &'static str = "preserved";

    fn Helper() -> &'static str {
        Self::LABEL
    }

    #[tool]
    async fn ping(&self) -> String {
        Self::Helper().to_string()
    }
}

fn main() {
    assert_eq!(PreservedImpl::Helper(), "preserved");
}
