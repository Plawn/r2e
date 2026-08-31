//! `WorkerCollector` — the sharded workers' standard metrics, straight from a
//! [`WorkerSet`].
//!
//! ```rust,ignore
//! let workers = WorkerSet::new();
//! AppBuilder::new()
//!     .provide(workers.clone())
//!     .plugin(Prometheus::builder()
//!         .register(Box::new(WorkerCollector::new(workers)))
//!         .build())
//! ```
//!
//! Exported series (namespace = the plugin's):
//!
//! | metric | labels | meaning |
//! |---|---|---|
//! | `r2e_worker_state` | `worker`, `state` | 1 for the worker's current lifecycle state, 0 otherwise |
//! | `r2e_worker_cpu` | `worker` | effective CPU affinity, `-1` when none |
//! | `r2e_worker_crossings_total` | `worker`, `origin` (`local`/`remote`) | mailbox deliveries attributed to the worker |
//! | `r2e_worker_mailbox_depth` | `worker` | messages queued, not yet received |
//! | `r2e_worker_mailbox_sends_total` | `worker` | total messages sent to the worker |
//! | `r2e_worker_mailbox_wait_seconds_total` | `worker` | cumulative queued time |
//! | `r2e_workers` | — | configured worker count |

use std::sync::Mutex;

use prometheus::core::{Collector, Desc};
use prometheus::proto::MetricFamily;
use prometheus::{IntCounterVec, IntGauge, IntGaugeVec, Opts};
use r2e_core::runtime::worker_set::{WorkerSet, WorkerSnapshot, WorkerState};

/// Prometheus collector over a [`WorkerSet`]. Register it through
/// [`PrometheusBuilder::register`](crate::PrometheusBuilder::register) or
/// [`PrometheusRegistry::register`](crate::PrometheusRegistry::register).
pub struct WorkerCollector {
    set: WorkerSet,
    workers: IntGauge,
    state: IntGaugeVec,
    cpu: IntGaugeVec,
    crossings: IntCounterVec,
    mailbox_depth: IntGaugeVec,
    mailbox_sends: IntCounterVec,
    mailbox_wait: prometheus::CounterVec,
    /// Last exported counter values, so the monotonic snapshot counters can
    /// be replayed as Prometheus counters (which only ever `inc`).
    last: Mutex<Vec<Last>>,
}

#[derive(Default, Clone, Copy)]
struct Last {
    local: u64,
    remote: u64,
    sends: u64,
    wait_nanos: u128,
}

impl std::fmt::Debug for WorkerCollector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerCollector")
            .field("workers", &self.set.workers())
            .finish()
    }
}

impl WorkerCollector {
    /// Metrics under the `r2e_worker_*` names.
    pub fn new(set: WorkerSet) -> Self {
        Self::with_namespace(set, "r2e")
    }

    /// Metrics under `<namespace>_worker_*`.
    pub fn with_namespace(set: WorkerSet, namespace: &str) -> Self {
        let opts = |name: &str, help: &str| Opts::new(name, help).namespace(namespace);
        Self {
            set,
            workers: IntGauge::with_opts(opts("workers", "Configured sharded worker count"))
                .expect("static metric opts"),
            state: IntGaugeVec::new(
                opts("worker_state", "1 for the worker's current lifecycle state"),
                &["worker", "state"],
            )
            .expect("static metric opts"),
            cpu: IntGaugeVec::new(
                opts(
                    "worker_cpu",
                    "Effective CPU affinity of the worker (-1 = none)",
                ),
                &["worker"],
            )
            .expect("static metric opts"),
            crossings: IntCounterVec::new(
                opts(
                    "worker_crossings_total",
                    "Mailbox deliveries to the worker, by origin (local = from the worker itself)",
                ),
                &["worker", "origin"],
            )
            .expect("static metric opts"),
            mailbox_depth: IntGaugeVec::new(
                opts(
                    "worker_mailbox_depth",
                    "Messages queued in the worker's mailbox",
                ),
                &["worker"],
            )
            .expect("static metric opts"),
            mailbox_sends: IntCounterVec::new(
                opts(
                    "worker_mailbox_sends_total",
                    "Messages sent to the worker's mailbox",
                ),
                &["worker"],
            )
            .expect("static metric opts"),
            mailbox_wait: prometheus::CounterVec::new(
                opts(
                    "worker_mailbox_wait_seconds_total",
                    "Cumulative time messages spent queued in the worker's mailbox",
                ),
                &["worker"],
            )
            .expect("static metric opts"),
            last: Mutex::new(Vec::new()),
        }
    }

    fn refresh(&self) {
        let snaps: Vec<WorkerSnapshot> = self.set.snapshot();
        self.workers.set(snaps.len() as i64);
        let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        if last.len() < snaps.len() {
            last.resize(snaps.len(), Last::default());
        }
        for snap in &snaps {
            let w = snap.id.to_string();
            for st in WorkerState::ALL {
                self.state
                    .with_label_values(&[&w, st.as_str()])
                    .set(i64::from(st == snap.state));
            }
            self.cpu
                .with_label_values(&[&w])
                .set(snap.cpu.map_or(-1, |c| c as i64));
            self.mailbox_depth
                .with_label_values(&[&w])
                .set(snap.mailbox_depth as i64);
            let prev = &mut last[snap.id];
            self.crossings
                .with_label_values(&[&w, "local"])
                .inc_by(snap.local_crossings.saturating_sub(prev.local));
            self.crossings
                .with_label_values(&[&w, "remote"])
                .inc_by(snap.remote_crossings.saturating_sub(prev.remote));
            self.mailbox_sends
                .with_label_values(&[&w])
                .inc_by(snap.mailbox_sends.saturating_sub(prev.sends));
            let wait_nanos = snap.mailbox_wait_total.as_nanos();
            self.mailbox_wait
                .with_label_values(&[&w])
                .inc_by(wait_nanos.saturating_sub(prev.wait_nanos) as f64 / 1e9);
            *prev = Last {
                local: snap.local_crossings,
                remote: snap.remote_crossings,
                sends: snap.mailbox_sends,
                wait_nanos,
            };
        }
    }
}

impl Collector for WorkerCollector {
    fn desc(&self) -> Vec<&Desc> {
        let mut d = self.workers.desc();
        d.extend(self.state.desc());
        d.extend(self.cpu.desc());
        d.extend(self.crossings.desc());
        d.extend(self.mailbox_depth.desc());
        d.extend(self.mailbox_sends.desc());
        d.extend(self.mailbox_wait.desc());
        d
    }

    fn collect(&self) -> Vec<MetricFamily> {
        self.refresh();
        let mut m = self.workers.collect();
        m.extend(self.state.collect());
        m.extend(self.cpu.collect());
        m.extend(self.crossings.collect());
        m.extend(self.mailbox_depth.collect());
        m.extend(self.mailbox_sends.collect());
        m.extend(self.mailbox_wait.collect());
        m
    }
}
