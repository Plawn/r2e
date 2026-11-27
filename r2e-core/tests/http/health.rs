use std::time::{Duration, Instant};

use r2e_core::builtins::health::{
    HealthBuilder, HealthCheckStatus, HealthIndicator, HealthIndicatorErased, HealthState,
    HealthStatus,
};

#[test]
fn health_status_is_up() {
    assert!(HealthStatus::Up.is_up());
}

#[test]
fn health_status_down_is_not_up() {
    assert!(!HealthStatus::Down("db unreachable".into()).is_up());
}

#[test]
fn health_builder_default() {
    // Verify default builder creates without error
    let _advanced = HealthBuilder::default().build();
}

struct AlwaysUp;
impl HealthIndicator for AlwaysUp {
    fn name(&self) -> &str {
        "up-check"
    }
    async fn check(&self) -> HealthStatus {
        HealthStatus::Up
    }
}

struct AlwaysDown;
impl HealthIndicator for AlwaysDown {
    fn name(&self) -> &str {
        "down-check"
    }
    async fn check(&self) -> HealthStatus {
        HealthStatus::Down("broken".into())
    }
}

#[test]
fn health_builder_collects_checks() {
    // Builder chain should accept multiple checks without panicking
    let _advanced = HealthBuilder::new()
        .check(AlwaysUp)
        .check(AlwaysDown)
        .build();
}

#[r2e_core::test]
async fn health_state_aggregate() {
    let state = HealthState::new(
        vec![
            Box::new(AlwaysUp) as Box<dyn HealthIndicatorErased>,
            Box::new(AlwaysDown) as Box<dyn HealthIndicatorErased>,
        ],
        None,
    );
    let response = state.aggregate().await;
    assert!(matches!(response.status, HealthCheckStatus::Down));
    assert_eq!(response.checks.len(), 2);
    assert!(matches!(response.checks[0].status, HealthCheckStatus::Up));
    assert!(matches!(response.checks[1].status, HealthCheckStatus::Down));
    assert_eq!(response.checks[1].reason.as_deref(), Some("broken"));
}

#[r2e_core::test]
async fn health_cache_returns_cached_result() {
    let state = HealthState::new(
        vec![Box::new(AlwaysUp) as Box<dyn HealthIndicatorErased>],
        Some(Duration::from_secs(60)),
    );
    // First call populates the cache
    let r1 = state.aggregate().await;
    assert!(matches!(r1.status, HealthCheckStatus::Up));
    // Second call reads from cache
    let r2 = state.aggregate().await;
    assert!(matches!(r2.status, HealthCheckStatus::Up));
    // Verify cache is populated
    let cache = state.cache.read().await;
    assert!(cache.is_some());
}

#[r2e_core::test]
async fn health_uptime_increases() {
    let state = HealthState {
        checks: vec![],
        start_time: Instant::now() - Duration::from_secs(5),
        cache_ttl: None,
        cache: tokio::sync::RwLock::new(None),
        readiness_names: std::collections::HashSet::new(),
    };
    let response = state.aggregate().await;
    assert!(response.uptime_seconds.unwrap() >= 5);
}

// ── Plugin-level: the `Health` plugin routes ──────────────────────────────

use r2e_core::builder::AppBuilder;
use r2e_core::builtins::Health;
use r2e_core::http::StatusCode;

use crate::support::send_get;

// ── Health plugin ────────────────────────────────────────────────────────

#[r2e_core::test]
async fn health_returns_200_ok() {
    let router = AppBuilder::new().plugin(Health).build_state().await.build();
    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

struct LivenessOnly;
impl HealthIndicator for LivenessOnly {
    fn name(&self) -> &str {
        "liveness-only"
    }
    async fn check(&self) -> HealthStatus {
        HealthStatus::Down("down but liveness-only".into())
    }
    fn affects_readiness(&self) -> bool {
        false
    }
}

#[r2e_core::test]
async fn advanced_health_all_up() {
    let router = AppBuilder::new()
        .plugin(Health::builder().check(AlwaysUp).build())
        .build_state()
        .await
        .build();
    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "UP");
}

#[r2e_core::test]
async fn advanced_health_one_down() {
    let router = AppBuilder::new()
        .plugin(Health::builder().check(AlwaysUp).check(AlwaysDown).build())
        .build_state()
        .await
        .build();
    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "DOWN");
}

#[r2e_core::test]
async fn advanced_health_liveness_always_ok() {
    let router = AppBuilder::new()
        .plugin(Health::builder().check(AlwaysDown).build())
        .build_state()
        .await
        .build();
    let (status, _body) = send_get(router, "/health/live").await;
    assert_eq!(status, StatusCode::OK);
}

#[r2e_core::test]
async fn advanced_health_readiness_filters() {
    // LivenessOnly check is down but doesn't affect readiness
    let router = AppBuilder::new()
        .plugin(
            Health::builder()
                .check(AlwaysUp)
                .check(LivenessOnly)
                .build(),
        )
        .build_state()
        .await
        .build();
    let (status, body) = send_get(router, "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "UP");
}

// ── Cache TTL and readiness filtering ────────────────────────────────────

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// An `Up` indicator that counts how many times it was actually run, so a
/// test can tell a cache hit from a re-run without sleeping.
struct Counting {
    name: &'static str,
    hits: Arc<AtomicUsize>,
    readiness: bool,
}

impl Counting {
    fn new(name: &'static str, readiness: bool) -> (Self, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        (
            Self {
                name,
                hits: hits.clone(),
                readiness,
            },
            hits,
        )
    }
}

impl HealthIndicator for Counting {
    fn name(&self) -> &str {
        self.name
    }
    async fn check(&self) -> HealthStatus {
        self.hits.fetch_add(1, Ordering::SeqCst);
        HealthStatus::Up
    }
    fn affects_readiness(&self) -> bool {
        self.readiness
    }
}

#[r2e_core::test]
async fn a_fresh_cache_entry_skips_rerunning_the_checks() {
    let (check, hits) = Counting::new("counted", true);
    let state = HealthState::new(
        vec![Box::new(check) as Box<dyn HealthIndicatorErased>],
        Some(Duration::from_secs(3600)),
    );
    state.aggregate().await;
    state.aggregate().await;
    state.aggregate().await;
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "cached result must be reused"
    );
}

#[r2e_core::test]
async fn an_expired_cache_entry_reruns_the_checks() {
    // A zero TTL is always expired (`elapsed < 0` never holds).
    let (check, hits) = Counting::new("counted", true);
    let state = HealthState::new(
        vec![Box::new(check) as Box<dyn HealthIndicatorErased>],
        Some(Duration::ZERO),
    );
    state.aggregate().await;
    state.aggregate().await;
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[r2e_core::test]
async fn without_a_ttl_every_call_reruns_the_checks() {
    let (check, hits) = Counting::new("counted", true);
    let state = HealthState::new(
        vec![Box::new(check) as Box<dyn HealthIndicatorErased>],
        None,
    );
    state.aggregate().await;
    state.aggregate().await;
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert!(state.cache.read().await.is_none());
}

#[r2e_core::test]
async fn readiness_reads_the_fresh_full_health_cache_filtered_by_name() {
    let (ready, ready_hits) = Counting::new("ready", true);
    let (live, live_hits) = Counting::new("live-only", false);
    let state = HealthState::new(
        vec![
            Box::new(ready) as Box<dyn HealthIndicatorErased>,
            Box::new(live) as Box<dyn HealthIndicatorErased>,
        ],
        Some(Duration::from_secs(3600)),
    );
    let full = state.aggregate().await;
    assert_eq!(full.checks.len(), 2);

    let readiness = state.aggregate_readiness().await;
    assert_eq!(
        ready_hits.load(Ordering::SeqCst),
        1,
        "served from the cache"
    );
    assert_eq!(live_hits.load(Ordering::SeqCst), 1, "served from the cache");
    assert!(matches!(readiness.status, HealthCheckStatus::Up));
    let names: Vec<&str> = readiness.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["ready"]);
}

#[r2e_core::test]
async fn readiness_with_an_expired_cache_reruns_only_readiness_checks() {
    let (ready, ready_hits) = Counting::new("ready", true);
    let (live, live_hits) = Counting::new("live-only", false);
    let state = HealthState::new(
        vec![
            Box::new(ready) as Box<dyn HealthIndicatorErased>,
            Box::new(live) as Box<dyn HealthIndicatorErased>,
        ],
        Some(Duration::ZERO),
    );
    state.aggregate().await;
    let readiness = state.aggregate_readiness().await;
    assert_eq!(ready_hits.load(Ordering::SeqCst), 2);
    assert_eq!(live_hits.load(Ordering::SeqCst), 1);
    let names: Vec<&str> = readiness.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["ready"]);
}

#[r2e_core::test]
async fn indicators_affect_readiness_by_default() {
    // `AlwaysDown` does not override `affects_readiness`: it must count.
    let state = HealthState::new(
        vec![
            Box::new(AlwaysDown) as Box<dyn HealthIndicatorErased>,
            Box::new(LivenessOnly) as Box<dyn HealthIndicatorErased>,
        ],
        None,
    );
    assert!(state.readiness_names.contains("down-check"));
    assert!(!state.readiness_names.contains("liveness-only"));

    let readiness = state.aggregate_readiness().await;
    assert!(matches!(readiness.status, HealthCheckStatus::Down));
    let names: Vec<&str> = readiness.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["down-check"]);
    assert_eq!(readiness.checks[0].reason.as_deref(), Some("broken"));
}

#[test]
fn the_erased_indicator_forwards_affects_readiness() {
    let default: Box<dyn HealthIndicatorErased> = Box::new(AlwaysUp);
    let liveness: Box<dyn HealthIndicatorErased> = Box::new(LivenessOnly);
    assert!(default.affects_readiness());
    assert!(!liveness.affects_readiness());
}

#[r2e_core::test]
async fn a_default_indicator_that_is_down_fails_the_readiness_route() {
    let router = AppBuilder::new()
        .plugin(Health::builder().check(AlwaysUp).check(AlwaysDown).build())
        .build_state()
        .await
        .build();
    let (status, body) = send_get(router, "/health/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "DOWN");
}

#[r2e_core::test]
async fn the_builder_cache_ttl_caches_the_health_route() {
    let (check, hits) = Counting::new("counted", true);
    let router = AppBuilder::new()
        .plugin(
            Health::builder()
                .check(check)
                .cache_ttl(Duration::from_secs(3600))
                .build(),
        )
        .build_state()
        .await
        .build();
    let (status, _) = send_get(router.clone(), "/health").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
