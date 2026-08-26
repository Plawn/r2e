//! A controller `#[on_start]` may take only `&self` (parity with beans).

use r2e::prelude::*;

#[controller]
pub struct Svc {}

#[routes]
impl Svc {
    #[on_start]
    async fn warm(&self, _extra: u32) {}
}

fn main() {}
