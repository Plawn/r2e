//! Shared-nothing service on sharded workers + control-plane aggregation.
//!
//! ```text
//! cargo run -p example-worker-udp
//! # in another shell:
//! echo hello | nc -u -w1 127.0.0.1 4433      # → "shard=3 n=1 hello"
//! curl 127.0.0.1:3000/ping                    # HTTP is served by the same workers
//! curl 127.0.0.1:3000/whoami                  # which worker answered
//! curl 127.0.0.1:3000/stats                   # per-shard datagram counts, aggregated
//! curl 127.0.0.1:3000/workers                 # WorkerSet lifecycle + crossing counters
//! curl 127.0.0.1:3000/metrics                 # r2e_worker_* Prometheus series
//! ```
//!
//! What this demonstrates (task #990 — the multi-worker API):
//!
//! - **Worker-local state** — `AppBuilder::worker_local` builds one
//!   `RefCell<ShardStats>` per worker. The handle `WorkerLocal<_>` is an
//!   ordinary `#[inject]` bean; the values never leave their worker (no
//!   `Send`, no atomics, no locks), and reading one from the wrong thread
//!   panics naming the thread it was asked from.
//! - **Worker-affine ingress** — `reuseport_udp` + `WorkerContext::adopt_udp`
//!   give each worker its own `SO_REUSEPORT` UDP socket on the same port. The
//!   same shape carries a per-worker QUIC endpoint.
//! - **Explicit crossings** — `Mailboxes<Cmd>` is the only way state crosses a
//!   worker boundary. `/stats` (any worker) and the `#[scheduled]` aggregator
//!   (control plane) ask every shard for its counters; every such hop shows up
//!   in `WorkerSet` as a *remote crossing*, so "shared-nothing on the hot path"
//!   is a number you can read at `/workers` or `/metrics`, not a belief.
//! - **Aggregated lifecycle** — the provided `WorkerSet` follows every worker
//!   (`starting → ready → serving → draining → … → exited`), `WorkerHealth`
//!   turns it into a readiness indicator, `WorkerCollector` exports it.
//! - **Identity as data** — `WorkerInfo` is an extractor: `/whoami` reports the
//!   worker that ran the request.

use std::cell::{Cell, RefCell};

use r2e::http::Json;
use r2e::prelude::*;
use r2e::r2e_executor::Executor;
use r2e::r2e_prometheus::{Prometheus, WorkerCollector};
use r2e::r2e_scheduler::Scheduler;
use r2e::rt::sync::oneshot;
use r2e::rt::{CancelToken, JobHandle, UdpSocket};
use r2e::runtime::ingress::reuseport_udp;
use r2e::runtime::worker::{BoxError, LocalBoxFuture, WorkerContext, WorkerService};
use r2e::runtime::worker_set::WorkerSnapshot;
use serde::Serialize;

// ── Worker-local state ──────────────────────────────────────────────────

/// Per-shard counters. Plain `Cell`s: only the owning worker touches them.
#[derive(Default)]
struct ShardStats {
    datagrams: Cell<u64>,
    bytes: Cell<u64>,
}

#[derive(Serialize, Clone, Copy, Debug)]
pub struct ShardReport {
    worker: usize,
    datagrams: u64,
    bytes: u64,
}

// ── Crossings ───────────────────────────────────────────────────────────

/// Everything that crosses into a worker goes through this enum.
pub enum Cmd {
    Report(oneshot::Sender<ShardReport>),
    Reset,
}

// ── The shard-local UDP service ─────────────────────────────────────────

struct ShardEcho {
    worker: usize,
    echo_loop: Option<JobHandle<()>>,
    mail_loop: Option<JobHandle<()>>,
}

impl ShardEcho {
    async fn start(
        worker: WorkerContext,
        port: u16,
        stats: WorkerLocal<RefCell<ShardStats>>,
        mail: Mailboxes<Cmd>,
    ) -> Result<Self, BoxError> {
        let addr: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
        // Worker-affine socket, or a visible error if the platform cannot.
        let sock = worker.adopt_udp(reuseport_udp(addr)?)?;
        let id = worker.id();
        tracing::info!(worker = id, cpu = ?worker.cpu(), port, "shard UDP echo ready");

        let echo_loop = worker.spawn_local(echo_loop(id, sock, stats.clone(), worker.shutdown()));

        // Mailbox: the only door into this shard's state.
        let mut inbox = mail.attach(&worker)?;
        let mail_loop = worker.spawn_local(async move {
            while let Some(cmd) = inbox.recv().await {
                match cmd {
                    Cmd::Report(reply) => {
                        let report = stats.with(|s| ShardReport {
                            worker: id,
                            datagrams: s.borrow().datagrams.get(),
                            bytes: s.borrow().bytes.get(),
                        });
                        let _ = reply.send(report);
                    }
                    Cmd::Reset => stats.with(|s| {
                        let s = s.borrow();
                        s.datagrams.set(0);
                        s.bytes.set(0);
                    }),
                }
            }
        });
        Ok(Self {
            worker: id,
            echo_loop: Some(echo_loop),
            mail_loop: Some(mail_loop),
        })
    }
}

async fn echo_loop(
    worker: usize,
    sock: UdpSocket,
    stats: WorkerLocal<RefCell<ShardStats>>,
    shutdown: CancelToken,
) {
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
        // Hot path: worker-local, zero crossings.
        let count = stats.with(|s| {
            let s = s.borrow();
            s.datagrams.set(s.datagrams.get() + 1);
            s.bytes.set(s.bytes.get() + n as u64);
            s.datagrams.get()
        });
        let reply = format!(
            "shard={worker} n={count} {}",
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
            if let Some(h) = self.echo_loop.take() {
                let _ = h.await;
            }
            if let Some(h) = self.mail_loop.take() {
                h.abort();
            }
            tracing::info!(worker = self.worker, "shard UDP echo stopped");
        })
    }
}

// ── Control plane: aggregation off the hot path ─────────────────────────

#[derive(Serialize)]
struct Stats {
    total_datagrams: u64,
    total_bytes: u64,
    shards: Vec<ShardReport>,
}

async fn collect(mail: &Mailboxes<Cmd>) -> Stats {
    let shards: Vec<ShardReport> = mail
        .ask_all(Cmd::Report)
        .await
        .into_iter()
        .filter_map(Result::ok)
        .collect();
    Stats {
        total_datagrams: shards.iter().map(|s| s.datagrams).sum(),
        total_bytes: shards.iter().map(|s| s.bytes).sum(),
        shards,
    }
}

/// Periodic aggregator: runs on the control plane, never on a worker's hot
/// path. Every tick is `workers` remote crossings — visible at `/workers`.
#[controller]
pub struct StatsAggregator {
    #[inject]
    mail: Mailboxes<Cmd>,
}

#[routes]
impl StatsAggregator {
    #[scheduled(every = 10)]
    async fn log_totals(&self) {
        let stats = collect(&self.mail).await;
        tracing::info!(
            from = %WorkerInfo::current_or_control_plane(),
            datagrams = stats.total_datagrams,
            bytes = stats.total_bytes,
            shards = stats.shards.len(),
            "udp totals"
        );
    }
}

// ── HTTP ────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct WhoAmI {
    worker: String,
    id: usize,
    workers: usize,
    role: String,
}

#[controller(path = "/")]
pub struct StatsController {
    #[inject]
    mail: Mailboxes<Cmd>,
    #[inject]
    workers: WorkerSet,
}

#[routes]
impl StatsController {
    #[get("/ping")]
    async fn ping(&self) -> &'static str {
        "pong"
    }

    /// Which worker served this request.
    #[get("/whoami")]
    async fn whoami(&self, info: WorkerInfo) -> Json<WhoAmI> {
        Json(WhoAmI {
            worker: info.to_string(),
            id: info.id(),
            workers: info.workers(),
            role: info.role().to_string(),
        })
    }

    /// Aggregated shard counters (one remote crossing per shard — including
    /// the one that serves this request, which is a *local* crossing).
    #[get("/stats")]
    async fn stats(&self) -> Json<Stats> {
        Json(collect(&self.mail).await)
    }

    #[post("/stats/reset")]
    async fn reset(&self) -> Json<usize> {
        Json(self.mail.broadcast_with(|| Cmd::Reset).await)
    }

    /// Lifecycle + crossing counters straight from the `WorkerSet`.
    #[get("/workers")]
    async fn workers(&self) -> Json<Vec<WorkerSnapshot>> {
        Json(self.workers.snapshot())
    }
}

pub struct UdpApp;

impl App for UdpApp {
    type Env = ();

    async fn setup() -> Result<(), BootError> {
        Ok(())
    }

    async fn build(b: AppBuilder, _env: ()) -> Result<impl BootableApp, BootError> {
        Ok({
            let port: u16 = std::env::var("UDP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(4433);
            let workers = WorkerSet::new();
            let mail: Mailboxes<Cmd> = Mailboxes::new(workers.clone(), 64);
            let stats: WorkerLocal<RefCell<ShardStats>> =
                WorkerLocal::new(|_worker| async { Ok(RefCell::new(ShardStats::default())) });
            let installer = stats.clone();
            let (svc_stats, svc_mail) = (stats.clone(), mail.clone());

            b.load_config::<()>()
                .provide(workers.clone())
                .provide(mail)
                // `WorkerLocal` as a bean: `.worker_local(factory)` is the one-liner
                // when the handle is not needed elsewhere; here the UDP service
                // reads it too, so we provide the same handle explicitly.
                .provide(stats)
                .per_worker_service(move |ctx| {
                    let installer = installer.clone();
                    async move { installer.install(ctx).await }
                })
                .per_worker_service(move |worker| {
                    ShardEcho::start(worker, port, svc_stats.clone(), svc_mail.clone())
                })
                .plugin(Scheduler)
                .plugin(Executor)
                .plugin(Health::builder().build())
                .plugin(WorkerHealth::new())
                .plugin(
                    Prometheus::builder()
                        .endpoint("/metrics")
                        .register(Box::new(WorkerCollector::new(workers)))
                        .build(),
                )
                .build_state()
                .await
                .register_controller::<StatsController>()
                .register_controller::<StatsAggregator>()
        })
    }
}

#[r2e::main]
async fn main() {
    r2e::launch!(UdpApp).await.unwrap();
}
