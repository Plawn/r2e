//! The deprecated `#[mcp_routes]` alias still expands (through the same
//! dispatcher as `#[routes]`). This is the one pass case kept on the alias.
//! The crate denies `deprecated` and the module allows it: the deprecation
//! lint fires at the macro path, so an `#[allow(deprecated)]` on the impl
//! itself does not silence it — a module (or crate) level one does.

#![deny(non_snake_case)]
#![deny(deprecated)]

mod legacy {
    #![allow(deprecated)]

    use r2e::prelude::*;

    #[controller]
    pub struct PreservedImpl;

    #[mcp_routes]
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
}

fn main() {
    assert_eq!(legacy::PreservedImpl::Helper(), "preserved");
}
