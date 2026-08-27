//! `#[on_start]` startup observers on a `#[bean]` impl: sync + async, default
//! and explicit (including negative) `order`, several hooks per impl.

use r2e::prelude::*;

#[derive(Clone)]
pub struct Warmer;

#[bean]
impl Warmer {
    pub fn new() -> Self {
        Self
    }

    #[on_start]
    async fn warm(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    #[on_start(order = -10)]
    fn very_early(&self) {}

    #[on_start(order = 100)]
    async fn late(&self) {}
}

fn assert_on_start<T: r2e::r2e_core::OnStart>() {}

fn main() {
    assert_on_start::<Warmer>();
}
