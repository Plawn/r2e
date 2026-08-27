//! `#[intercept]` has no dispatch wrapper on a plain `#[on_start]` hook.

use r2e::prelude::*;

#[controller]
pub struct Svc {}

#[routes]
impl Svc {
    #[on_start]
    #[intercept(r2e::r2e_utils::Logged::info())]
    async fn warm(&self) {}
}

fn main() {}
