//! `order` sequences the after-drain lane only. On the early lane every
//! service is cancelled at once, so an `order` there would be a silent no-op
//! the author mistakes for sequencing — reject it.
use r2e::prelude::*;
use r2e::rt::CancelToken;

#[derive(BackgroundService)]
#[service(order = 2)]
pub struct Poller;

impl Poller {
    async fn run(&self, shutdown: CancelToken) {
        shutdown.cancelled().await;
    }
}

fn main() {}
