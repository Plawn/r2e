//! `#[producer(start)]` runs its output as a background service, so the
//! *service's* own `#[inject]` fields are demanded from the graph too — not
//! just the producer function's parameters.
//!
//! Here the producer takes no parameters, but the service it produces reads a
//! bean the app never provided. `#[producer]` folds
//! `<Output as ServiceComponent>::Deps` into the producer's registration deps,
//! so this is an `AllSatisfied` failure at `build_state()`. Without that fold
//! the app compiled, booted, and panicked inside
//! `ServiceComponent::from_context` when the service task started.

use r2e::prelude::*;
use r2e::rt::CancelToken;

/// The bean the service needs — deliberately never provided.
#[derive(Clone)]
pub struct MetricsSink;

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
fn make_metrics_exporter() -> MetricsExporter {
    MetricsExporter { sink: MetricsSink }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .register::<MakeMetricsExporter>()
            .build_state()
            .await
    };
}
