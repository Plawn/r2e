//! Phase 0 of plans/json-codec-containment.md: is a SIMD codec worth a
//! feature? `serde_json` vs `sonic-rs` on the two shapes the framework
//! actually serializes — a small single struct (one entity, ~200 B) and a
//! page of them (~4 KiB), both directions.
//!
//! Run: `cargo bench -p r2e-http --bench json`

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
struct User {
    id: u64,
    name: String,
    email: String,
    active: bool,
    score: f64,
    tags: Vec<String>,
}

fn user(i: u64) -> User {
    User {
        id: i,
        name: format!("user-{i}"),
        email: format!("user{i}@example.com"),
        active: i % 2 == 0,
        score: i as f64 * 1.5,
        tags: vec!["alpha".into(), "beta".into(), "gamma".into()],
    }
}

fn bench(c: &mut Criterion) {
    let one = user(1);
    let page: Vec<User> = (0..32).map(user).collect();
    let one_bytes = serde_json::to_vec(&one).unwrap();
    let page_bytes = serde_json::to_vec(&page).unwrap();

    let mut g = c.benchmark_group("to_vec/one");
    g.throughput(Throughput::Bytes(one_bytes.len() as u64));
    g.bench_function("serde_json", |b| {
        b.iter(|| serde_json::to_vec(black_box(&one)).unwrap())
    });
    g.bench_function("sonic_rs", |b| {
        b.iter(|| sonic_rs::to_vec(black_box(&one)).unwrap())
    });
    g.finish();

    let mut g = c.benchmark_group("to_vec/page");
    g.throughput(Throughput::Bytes(page_bytes.len() as u64));
    g.bench_function("serde_json", |b| {
        b.iter(|| serde_json::to_vec(black_box(&page)).unwrap())
    });
    g.bench_function("sonic_rs", |b| {
        b.iter(|| sonic_rs::to_vec(black_box(&page)).unwrap())
    });
    g.finish();

    let mut g = c.benchmark_group("from_slice/one");
    g.throughput(Throughput::Bytes(one_bytes.len() as u64));
    g.bench_function("serde_json", |b| {
        b.iter(|| serde_json::from_slice::<User>(black_box(&one_bytes)).unwrap())
    });
    g.bench_function("sonic_rs", |b| {
        b.iter(|| sonic_rs::from_slice::<User>(black_box(&one_bytes)).unwrap())
    });
    g.finish();

    let mut g = c.benchmark_group("from_slice/page");
    g.throughput(Throughput::Bytes(page_bytes.len() as u64));
    g.bench_function("serde_json", |b| {
        b.iter(|| serde_json::from_slice::<Vec<User>>(black_box(&page_bytes)).unwrap())
    });
    g.bench_function("sonic_rs", |b| {
        b.iter(|| sonic_rs::from_slice::<Vec<User>>(black_box(&page_bytes)).unwrap())
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
