//! A controller `#[post_construct]` may take only `&self` (parity with beans).

use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller]
pub struct Svc {}

#[routes]
impl Svc {
    #[post_construct]
    async fn init(&self, _extra: u32) {}
}

fn main() {}
