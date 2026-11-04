//! A `#[completion]` method no member references is dead code — refused.

use r2e::prelude::*;

#[controller]
pub struct Unwired {}

#[routes]
impl Unwired {
    #[completion]
    async fn orphan(&self, _c: Completion) -> Vec<String> {
        Vec::new()
    }
}

fn main() {}
