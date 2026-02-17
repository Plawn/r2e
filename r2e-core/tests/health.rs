use std::time::{Duration, Instant};

use r2e_core::health::{
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
    fn name(&self) -> &str { "up-check" }
    fn check(&self) -> impl std::future::Future<Output = HealthStatus> + Send {
        async { HealthStatus::Up }
    }
}

struct AlwaysDown;
impl HealthIndicator for AlwaysDown {
    fn name(&self) -> &str { "down-check" }
    fn check(&self) -> impl std::future::Future<Output = HealthStatus> + Send {
        async { HealthStatus::Down("broken".into()) }
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

#[tokio::test]
async fn health_state_aggregate() {
    let state = HealthState {
        checks: vec![
            Box::new(AlwaysUp) as Box<dyn HealthIndicatorErased>,
            Box::new(AlwaysDown) as Box<dyn HealthIndicatorErased>,
        ],
        start_time: Instant::now(),
        cache_ttl: None,
        cache: tokio::sync::RwLock::new(None),
    };
    let response = state.aggregate().await;
    assert!(matches!(response.status, HealthCheckStatus::Down));
    assert_eq!(response.checks.len(), 2);
    assert!(matches!(response.checks[0].status, HealthCheckStatus::Up));
    assert!(matches!(response.checks[1].status, HealthCheckStatus::Down));
    assert_eq!(response.checks[1].reason.as_deref(), Some("broken"));
}

#[tokio::test]
async fn health_cache_returns_cached_result() {
    let state = HealthState {
        checks: vec![Box::new(AlwaysUp) as Box<dyn HealthIndicatorErased>],
        start_time: Instant::now(),
        cache_ttl: Some(Duration::from_secs(60)),
        cache: tokio::sync::RwLock::new(None),
    };
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

#[tokio::test]
async fn health_uptime_increases() {
    let state = HealthState {
        checks: vec![],
        start_time: Instant::now() - Duration::from_secs(5),
        cache_ttl: None,
        cache: tokio::sync::RwLock::new(None),
    };
    let response = state.aggregate().await;
    assert!(response.uptime_seconds.unwrap() >= 5);
}
