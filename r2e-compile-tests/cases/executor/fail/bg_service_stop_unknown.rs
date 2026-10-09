//! `#[service(stop = "…")]` accepts exactly two phases; a typo must not fall
//! back to the early lane silently (the sink would then lose writes at
//! shutdown, which is the bug the attribute exists to prevent).
use r2e::prelude::*;
use r2e::rt::CancelToken;

#[derive(BackgroundService)]
#[service(stop = "late")]
pub struct Sink;

impl Sink {
    async fn run(&self, shutdown: CancelToken) {
        shutdown.cancelled().await;
    }
}

fn main() {}
