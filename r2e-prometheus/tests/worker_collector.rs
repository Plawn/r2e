use prometheus::core::Collector;
use prometheus::{Encoder, TextEncoder};
use r2e_core::runtime::worker_set::{WorkerSet, WorkerState};
use r2e_prometheus::WorkerCollector;

fn render(c: &WorkerCollector) -> String {
    let mut buf = Vec::new();
    TextEncoder::new().encode(&c.collect(), &mut buf).unwrap();
    String::from_utf8(buf).unwrap()
}

#[test]
fn exports_state_cpu_crossings_and_mailbox_series() {
    let set = WorkerSet::new();
    set.configure(2);
    let s1 = set.slot(1).unwrap();
    s1.set_state(WorkerState::Serving);
    s1.set_cpu(Some(3));
    s1.record_crossing(true);
    s1.record_crossing(false);
    s1.record_crossing(false);
    s1.mailbox_enqueued();
    s1.mailbox_enqueued();
    s1.mailbox_dequeued(std::time::Duration::from_millis(250));

    let c = WorkerCollector::new(set.clone());
    let out = render(&c);
    assert!(out.contains("r2e_workers 2"), "{out}");
    assert!(out.contains(r#"r2e_worker_state{state="serving",worker="1"} 1"#), "{out}");
    assert!(out.contains(r#"r2e_worker_state{state="unstarted",worker="1"} 0"#), "{out}");
    assert!(out.contains(r#"r2e_worker_state{state="unstarted",worker="0"} 1"#), "{out}");
    assert!(out.contains(r#"r2e_worker_cpu{worker="1"} 3"#), "{out}");
    assert!(out.contains(r#"r2e_worker_cpu{worker="0"} -1"#), "{out}");
    assert!(out.contains(r#"r2e_worker_crossings_total{origin="local",worker="1"} 1"#), "{out}");
    assert!(out.contains(r#"r2e_worker_crossings_total{origin="remote",worker="1"} 2"#), "{out}");
    assert!(out.contains(r#"r2e_worker_mailbox_depth{worker="1"} 1"#), "{out}");
    assert!(out.contains(r#"r2e_worker_mailbox_sends_total{worker="1"} 2"#), "{out}");
    assert!(out.contains(r#"r2e_worker_mailbox_wait_seconds_total{worker="1"} 0.25"#), "{out}");

    // Counters are replayed as deltas: a second scrape does not double-count.
    s1.record_crossing(false);
    let out = render(&c);
    assert!(out.contains(r#"r2e_worker_crossings_total{origin="remote",worker="1"} 3"#), "{out}");
    assert!(out.contains(r#"r2e_worker_mailbox_sends_total{worker="1"} 2"#), "{out}");
}

#[test]
fn registers_on_a_registry_and_honours_namespace() {
    let set = WorkerSet::new();
    set.configure(1);
    let reg = prometheus::Registry::new();
    reg.register(Box::new(WorkerCollector::with_namespace(set, "app")))
        .unwrap();
    let mut buf = Vec::new();
    TextEncoder::new().encode(&reg.gather(), &mut buf).unwrap();
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("app_workers 1"), "{out}");
    assert!(out.contains(r#"app_worker_state{state="unstarted",worker="0"} 1"#), "{out}");
}
