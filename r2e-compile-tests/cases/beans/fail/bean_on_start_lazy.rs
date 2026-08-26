//! `#[bean(lazy)]` has no registration-time instance to observe from, so it
//! cannot carry `#[on_start]` (same rule as `#[post_construct]`).

use r2e::prelude::*;

#[derive(Clone)]
pub struct Warmer;

#[bean(lazy)]
impl Warmer {
    pub fn new() -> Self {
        Self
    }

    #[on_start]
    async fn warm(&self) {}
}

fn main() {}
