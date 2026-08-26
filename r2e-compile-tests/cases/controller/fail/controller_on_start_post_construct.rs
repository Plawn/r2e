//! `#[on_start]` and `#[post_construct]` are distinct phases — one method
//! cannot be both.

use r2e::prelude::*;

#[controller]
pub struct Svc {}

#[routes]
impl Svc {
    #[post_construct]
    #[on_start]
    async fn init(&self) {}
}

fn main() {}
