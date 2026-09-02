//! Scaffolding for `llm/runtime-facade.md`.

use std::cell::RefCell;
use std::rc::Rc;

use r2e::rt;

/// The blocking work handed to `rt::spawn_blocking`.
pub fn heavy_sync() {}

/// A SO_REUSEPORT UDP socket bound per worker (socket2 in a real app).
pub fn bind_reuseport_udp(_port: u16) -> std::io::Result<std::net::UdpSocket> {
    std::net::UdpSocket::bind("0.0.0.0:0")
}

/// The per-shard receive loop the worker service spawns onto its `LocalSet`.
pub async fn run(_sock: rt::UdpSocket, _hits: Rc<RefCell<u64>>, stop: rt::CancelToken) {
    stop.cancelled().await;
}
