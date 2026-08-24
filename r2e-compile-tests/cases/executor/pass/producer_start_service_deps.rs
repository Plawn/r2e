//! The accepted half of `producer_start_service_missing_dep`: the same
//! `#[producer(start)]` service, with the bean it reads provided.
//!
//! Both dependency sources are folded into the producer's registration deps
//! and both are satisfied here — the producer function's own parameter
//! (`ExportConfig`) and the produced service's `#[inject]` field
//! (`MetricsSink`, read by the derived `ServiceComponent::from_context`, never
//! by the producer).

use r2e::prelude::*;
use r2e::rt::CancelToken;

#[derive(Clone)]
pub struct MetricsSink;

#[derive(Clone)]
pub struct ExportConfig;

#[derive(BackgroundService, Clone)]
pub struct MetricsExporter {
    #[inject]
    sink: MetricsSink,
}

impl MetricsExporter {
    async fn run(&self, shutdown: CancelToken) {
        let _ = &self.sink;
        shutdown.cancelled().await;
    }
}

#[producer(start)]
fn make_metrics_exporter(_cfg: ExportConfig) -> MetricsExporter {
    MetricsExporter { sink: MetricsSink }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .provide(MetricsSink)
            .provide(ExportConfig)
            .register::<MakeMetricsExporter>()
            .build_state()
            .await
    };
}
