//! A controller `#[pre_destroy]` may take only `&self` (parity with beans).

use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller]
pub struct Svc {}

#[routes]
impl Svc {
    #[pre_destroy]
    async fn shutdown(&self, _extra: u32) {}
}

fn main() {}
