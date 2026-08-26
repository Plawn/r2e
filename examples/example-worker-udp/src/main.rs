//! Per-worker services: one shard-local UDP echo socket per HTTP worker.
//!
//! ```text
//! cargo run -p example-worker-udp
//! # in another shell:
//! echo hello | nc -u -w1 127.0.0.1 4433      # → "shard=3 n=1 hello"
//! curl 127.0.0.1:3000/ping                    # HTTP is served by the same workers
//! ```
//!
//! What this demonstrates:
//! - `AppBuilder::per_worker_service` runs the factory **once per worker**, on
//!   the worker's own thread, before it accepts its first HTTP connection.
//! - The service owns `!Send` state (`Rc<RefCell<u64>>`), a UDP socket bound
//!   with `SO_REUSEPORT` on the *same* port in every worker, and a local task
//!   spawned with `WorkerContext::spawn_local` — none of it ever leaves the
//!   worker thread.
//! - Graceful shutdown (Ctrl-C): the worker's shutdown token stops the echo
//!   loop, then `WorkerService::shutdown` awaits it and reports the shard's
//!   datagram count.
//!
//! The socket is a plain `std::net::UdpSocket` configured with `socket2` before
//! being adopted into the worker runtime — the same pattern works for any
//! pre-bound socket (custom buffer sizes, cBPF steering, a Quinn endpoint over
//! that socket, …) without R2E knowing about the protocol on top.

use std::cell::RefCell;
use std::rc::Rc;

use r2e::prelude::*;
use r2e::rt::{CancelToken, JobHandle, UdpSocket};
use r2e::runtime::worker::{BoxError, LocalBoxFuture, WorkerContext, WorkerService};

/// Bind a UDP socket with `SO_REUSEPORT` so every worker can bind the same
/// port. Non-blocking is mandatory before `rt::UdpSocket::from_std`.
fn bind_reuseport_udp(port: u16) -> std::io::Result<std::net::UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let addr: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    sock.set_reuse_port(true)?;
    sock.bind(&addr.into())?;
    let std_sock: std::net::UdpSocket = sock.into();
    std_sock.set_nonblocking(true)?;
    Ok(std_sock)
}

/// The shard-local service: `!Send` on purpose.
struct ShardEcho {
    worker: usize,
    /// Datagrams handled by this shard — plain `Rc<RefCell>`, no atomics, no
    /// locks: only this worker thread ever touches it.
    count: Rc<RefCell<u64>>,
    echo_loop: Option<JobHandle<()>>,
}

impl ShardEcho {
    async fn start(worker: WorkerContext, port: u16) -> Result<Self, BoxError> {
        let std_sock = bind_reuseport_udp(port)?;
        // Adopting the socket must happen on the worker runtime — we are on it.
        let sock = UdpSocket::from_std(std_sock)?;
        let count = Rc::new(RefCell::new(0u64));
        let id = worker.id();
        tracing::info!(worker = id, cpu = ?worker.cpu(), port, "shard UDP echo ready");

        let echo_loop = worker.spawn_local(echo_loop(id, sock, Rc::clone(&count), worker.shutdown()));
        Ok(Self {
            worker: id,
            count,
            echo_loop: Some(echo_loop),
        })
    }
}

async fn echo_loop(worker: usize, sock: UdpSocket, count: Rc<RefCell<u64>>, shutdown: CancelToken) {
    let mut buf = vec![0u8; 2048];
    loop {
        let (n, peer) = r2e::rt::select! {
            _ = shutdown.cancelled() => break,
            res = sock.recv_from(&mut buf) => match res {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!(worker, error = %e, "udp recv failed");
                    continue;
                }
            },
        };
        *count.borrow_mut() += 1;
        let reply = format!(
            "shard={worker} n={} {}",
            count.borrow(),
            String::from_utf8_lossy(&buf[..n])
        );
        if let Err(e) = sock.send_to(reply.as_bytes(), peer).await {
            tracing::warn!(worker, error = %e, "udp send failed");
        }
    }
}

impl WorkerService for ShardEcho {
    fn shutdown(mut self: Box<Self>) -> LocalBoxFuture<'static, ()> {
        Box::pin(async move {
            // The shutdown token already fired (HTTP is drained by now); wait
            // for the echo loop to observe it and exit.
            if let Some(h) = self.echo_loop.take() {
                let _ = h.await;
            }
            tracing::info!(
                worker = self.worker,
                datagrams = *self.count.borrow(),
                "shard UDP echo stopped"
            );
        })
    }
}

#[controller(path = "/")]
pub struct PingController;

#[routes]
impl PingController {
    #[get("/ping")]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

pub struct UdpApp;

impl App for UdpApp {
    type Env = ();

    // Tracing is installed by the default `Tracing` builtin plugin.
    async fn setup() {}

    async fn build(b: AppBuilder, _env: ()) -> impl BootableApp {
        // UDP port: `UDP_PORT` env var, default 4433.
        let port: u16 = std::env::var("UDP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(4433);
        b.load_config::<()>()
            .build_state()
            .await
            .register_controller::<PingController>()
            // The factory is shared by all workers (Send + Sync); everything
            // it builds stays on the worker that called it.
            .per_worker_service(move |worker| ShardEcho::start(worker, port))
    }
}

#[r2e::main]
async fn main() {
    r2e::launch!(UdpApp).await.unwrap();
}
