//! A full block and its short tail must both reject an absent dependency.
use r2e::prelude::*;

#[derive(Clone)]
struct Missing;

#[derive(Clone)]
struct Consumer;

#[bean]
impl Consumer {
    fn new(_missing: Missing) -> Self {
        Self
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .register::<Consumer>()
            .provide(0u8)
            .provide(0u16)
            .provide(0u32)
            .provide(0u64)
            .provide(0i8)
            .provide(0i16)
            .provide(0i32)
            .build_state()
            .await
    };
}
