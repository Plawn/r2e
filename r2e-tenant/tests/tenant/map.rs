//! `Tenanted<T>`: single flight, failure handling, negative caching, timeouts,
//! eviction and drain.
//!
//! These are the invariants documented on the type. Each one exists because the
//! naive implementation gets it wrong: a `DashMap<TenantId, T>` with
//! `entry().or_insert_with(create)` would create N times under load, cache the
//! first failure forever, let a hanging `create` park every waiter, and leak a
//! slot per made-up tenant id.

use std::sync::Arc;
use std::time::Duration;

use r2e_core::http::StatusCode;
use r2e_core::BeanContext;
use r2e_tenant::{TenantError, TenantId, Tenanted, TenantedSettings};

use crate::fixtures::{map_with, tid, Behaviour, Resource, ScriptedSource};

fn settings() -> TenantedSettings {
    TenantedSettings {
        max_active: 100,
        idle_ttl: None,
        create_timeout: Some(Duration::from_secs(5)),
        negative_ttl: None,
        max_negative: 64,
        statuses: r2e_tenant::TenantStatuses::default(),
    }
}

#[tokio::test]
async fn creates_once_and_caches() {
    let (map, source) = map_with(ScriptedSource::new(), settings());
    let acme = tid("acme");

    let first = map.get(&acme).await.unwrap();
    let second = map.get(&acme).await.unwrap();

    assert_eq!(first, second, "the cached resource must be reused");
    assert_eq!(source.creates(), 1);
    assert_eq!(map.metrics().created, 1);
    assert_eq!(map.metrics().hits, 1);
}

#[tokio::test]
async fn routes_each_tenant_to_its_own_resource() {
    let (map, _source) = map_with(ScriptedSource::new(), settings());

    let acme = map.get(&tid("acme")).await.unwrap();
    let globex = map.get(&tid("globex")).await.unwrap();

    assert_eq!(acme.tenant, "acme");
    assert_eq!(globex.tenant, "globex");
    assert_ne!(acme, globex);

    let mut active = map.active();
    active.sort();
    assert_eq!(
        active.iter().map(TenantId::as_str).collect::<Vec<_>>(),
        ["acme", "globex"]
    );
}

/// The core concurrency property: 50 requests arriving for a cold tenant open
/// **one** pool, not 50.
#[tokio::test]
async fn concurrent_first_requests_create_once() {
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Slow(Duration::from_millis(50))),
        settings(),
    );
    let acme = tid("acme");

    let mut tasks = Vec::new();
    for _ in 0..50 {
        let map = map.clone();
        let acme = acme.clone();
        tasks.push(tokio::spawn(async move { map.get(&acme).await }));
    }

    let mut resources = Vec::new();
    for task in tasks {
        resources.push(task.await.unwrap().expect("every waiter is served"));
    }

    assert_eq!(source.creates(), 1, "single flight");
    assert!(
        resources.windows(2).all(|pair| pair[0] == pair[1]),
        "every waiter must get the same resource"
    );
}

#[tokio::test]
async fn failures_are_not_cached_and_leave_no_slot() {
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::FailTimes(1)),
        settings(),
    );
    let acme = tid("acme");

    let err = map.get(&acme).await.unwrap_err();
    assert!(
        matches!(err, TenantError::Unavailable { .. }),
        "expected Unavailable, got {err:?}"
    );
    assert!(
        map.stats().is_empty(),
        "a failed creation must not leave a slot behind (hostile ids would accumulate)"
    );

    // The next request retries rather than replaying the cached failure.
    let resource = map.get(&acme).await.expect("retry succeeds");
    assert_eq!(resource.tenant, "acme");
    assert_eq!(source.creates(), 2);
    assert_eq!(map.metrics().create_failures, 1);
}

#[tokio::test]
async fn failure_maps_to_service_unavailable() {
    let (map, _source) = map_with(
        ScriptedSource::with_default(Behaviour::Fail("no route to host".into())),
        settings(),
    );

    let err = map.get(&tid("acme")).await.unwrap_err();
    assert!(err.to_string().contains("no route to host"), "{err}");
    let http = err.into_http_error(map.statuses());
    assert_eq!(http.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn unknown_tenant_is_reported_and_negatively_cached() {
    let mut settings = settings();
    settings.negative_ttl = Some(Duration::from_millis(60));
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Unknown),
        settings,
    );
    let ghost = tid("ghost");

    let err = map.get(&ghost).await.unwrap_err();
    assert!(matches!(err, TenantError::Unknown(_)), "{err:?}");
    assert_eq!(
        err.into_http_error(map.statuses()).status(),
        StatusCode::NOT_FOUND
    );

    // Second lookup is served from the negative cache: the directory is not
    // hammered by a client retrying a bad tenant.
    assert!(map.get(&ghost).await.is_err());
    assert_eq!(source.creates(), 1);
    assert_eq!(map.metrics().negative, 1);
}

#[tokio::test]
async fn negative_cache_expires_so_new_tenants_appear() {
    let mut settings = settings();
    settings.negative_ttl = Some(Duration::from_millis(30));
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Unknown),
        settings,
    );
    let fresh = tid("fresh");

    assert!(map.get(&fresh).await.is_err());
    assert_eq!(source.creates(), 1);

    // The tenant gets provisioned...
    source.set("fresh", Behaviour::Ok);
    assert!(
        map.get(&fresh).await.is_err(),
        "still negatively cached within the TTL"
    );

    tokio::time::sleep(Duration::from_millis(50)).await;
    let resource = map.get(&fresh).await.expect("retried after the TTL");
    assert_eq!(resource.tenant, "fresh");
    assert_eq!(
        map.metrics().negative,
        0,
        "a success must clear the negative entry"
    );
}

#[tokio::test]
async fn invalidate_clears_the_negative_cache_immediately() {
    let mut settings = settings();
    settings.negative_ttl = Some(Duration::from_secs(300));
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Unknown),
        settings,
    );
    let fresh = tid("fresh");

    assert!(map.get(&fresh).await.is_err());
    source.set("fresh", Behaviour::Ok);
    map.invalidate(&fresh);

    assert!(
        map.get(&fresh).await.is_ok(),
        "invalidate must let a just-provisioned tenant in without waiting out the TTL"
    );
}

#[tokio::test]
async fn slow_creation_times_out_as_gateway_timeout() {
    let mut settings = settings();
    settings.create_timeout = Some(Duration::from_millis(20));
    let (map, _source) = map_with(
        ScriptedSource::with_default(Behaviour::Slow(Duration::from_secs(30))),
        settings,
    );

    let err = map.get(&tid("acme")).await.unwrap_err();
    assert!(matches!(err, TenantError::Timeout(_)), "{err:?}");
    assert_eq!(
        err.into_http_error(map.statuses()).status(),
        StatusCode::GATEWAY_TIMEOUT
    );
    assert_eq!(map.metrics().timeouts, 1);
    assert!(
        map.stats().is_empty(),
        "a timed-out creation must not leave a slot behind"
    );
}

#[tokio::test]
async fn timeout_releases_every_waiter() {
    let mut settings = settings();
    settings.create_timeout = Some(Duration::from_millis(20));
    let (map, _source) = map_with(
        ScriptedSource::with_default(Behaviour::Slow(Duration::from_secs(30))),
        settings,
    );
    let acme = tid("acme");

    let mut tasks = Vec::new();
    for _ in 0..5 {
        let map = map.clone();
        let acme = acme.clone();
        tasks.push(tokio::spawn(async move { map.get(&acme).await }));
    }
    for task in tasks {
        // Whoever wins the cell hits the timeout; the others retry and hit it
        // too. Nobody parks forever behind a hanging `create`.
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("no waiter may outlive the create timeout")
            .unwrap();
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn idle_resources_are_evicted_and_disposed() {
    let mut settings = settings();
    settings.idle_ttl = Some(Duration::from_millis(20));
    let (map, source) = map_with(ScriptedSource::new(), settings);
    let acme = tid("acme");

    map.get(&acme).await.unwrap();
    assert_eq!(map.sweep().await.idle_evicted, 0, "not idle yet");

    tokio::time::sleep(Duration::from_millis(40)).await;
    let report = map.sweep().await;
    assert_eq!(report.idle_evicted, 1);
    assert_eq!(source.disposals(), ["acme"], "eviction must dispose");
    assert!(map.active().is_empty());

    // And a later request rebuilds it.
    map.get(&acme).await.unwrap();
    assert_eq!(source.creates(), 2);
}

#[tokio::test]
async fn max_active_evicts_least_recently_used() {
    let mut settings = settings();
    settings.max_active = 2;
    let (map, source) = map_with(ScriptedSource::new(), settings);

    // Distinct last-used timestamps: `last_used` has millisecond resolution.
    for tenant in ["first", "second", "third"] {
        map.get(&tid(tenant)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    map.sweep().await;

    let active: Vec<String> = map
        .active()
        .iter()
        .map(|id| id.as_str().to_string())
        .collect();
    assert_eq!(active.len(), 2, "the cap must hold: {active:?}");
    assert!(
        !active.contains(&"first".to_string()),
        "the least recently used tenant must be the victim: {active:?}"
    );
    assert!(
        source.disposals().contains(&"first".to_string()),
        "LRU eviction must dispose: {:?}",
        source.disposals()
    );
}

#[tokio::test]
async fn evict_disposes_and_reports_whether_anything_was_cached() {
    let (map, source) = map_with(ScriptedSource::new(), settings());
    let acme = tid("acme");

    assert!(!map.evict(&acme).await, "nothing cached yet");
    map.get(&acme).await.unwrap();
    assert!(map.evict(&acme).await);
    assert_eq!(source.disposals(), ["acme"]);
    assert_eq!(map.metrics().disposed, 1);
}

#[tokio::test]
async fn drain_disposes_every_tenant() {
    let (map, source) = map_with(ScriptedSource::new(), settings());
    for tenant in ["a", "b", "c"] {
        map.get(&tid(tenant)).await.unwrap();
    }

    map.drain().await;

    let mut disposed = source.disposals();
    disposed.sort();
    assert_eq!(disposed, ["a", "b", "c"]);
    assert!(map.active().is_empty());
}

#[tokio::test]
async fn peek_and_stats_expose_the_live_map() {
    let (map, _source) = map_with(ScriptedSource::new(), settings());
    let acme = tid("acme");

    assert_eq!(map.peek(&acme), None, "peek must not create");
    map.get(&acme).await.unwrap();
    assert!(map.peek(&acme).is_some());

    let stats = map.stats();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].tenant, acme);
    assert!(stats[0].ready);
}

#[tokio::test]
async fn preload_creates_up_front_and_reports_failures() {
    let (map, source) = map_with(
        ScriptedSource::new().on("broken", Behaviour::Fail("cold".into())),
        settings(),
    );

    let failures = map
        .preload([tid("warm"), tid("broken")])
        .await
        .into_iter()
        .map(|(tenant, _)| tenant.as_str().to_string())
        .collect::<Vec<_>>();

    assert_eq!(failures, ["broken"]);
    assert!(map.peek(&tid("warm")).is_some(), "warm tenant is ready");
    assert_eq!(source.creates(), 2);
}

#[tokio::test]
async fn an_unwired_map_names_the_missing_plugin() {
    let map: Tenanted<Resource> = Tenanted::unwired();

    let err = map.get(&tid("acme")).await.unwrap_err();
    assert!(matches!(err, TenantError::NoSource(_)), "{err:?}");
    assert!(err.is_bug());
    assert!(
        err.to_string().contains("PerTenant"),
        "the error must name the plugin to add: {err}"
    );
    assert_eq!(
        err.into_http_error(map.statuses()).status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn fallback_default_serves_unknown_tenants_and_is_never_disposed() {
    let source = ScriptedSource::with_default(Behaviour::Unknown);
    let shared = Resource {
        tenant: "shared".into(),
        generation: 999,
    };
    let map = Tenanted::new(
        Arc::new(source.clone()),
        Arc::new(BeanContext::empty()),
        settings(),
        Some(shared.clone()),
    );

    let resource = map
        .get(&tid("ghost"))
        .await
        .expect("an unknown tenant falls back to the app-scoped default");
    assert_eq!(resource, shared);
    assert_eq!(map.metrics().fallbacks, 1);

    // The fallback is app-scoped: it is not cached per tenant, so eviction and
    // drain never dispose of the bean the rest of the app is still using.
    assert!(map.active().is_empty());
    map.drain().await;
    assert!(
        source.disposals().is_empty(),
        "the shared default must never be disposed: {:?}",
        source.disposals()
    );
}

#[test]
fn sweep_interval_is_clamped() {
    let mut settings = TenantedSettings {
        idle_ttl: Some(Duration::from_millis(10)),
        ..TenantedSettings::default()
    };
    assert_eq!(settings.sweep_interval(), Duration::from_secs(1));

    settings.idle_ttl = Some(Duration::from_secs(3600));
    assert_eq!(settings.sweep_interval(), Duration::from_secs(60));

    settings.idle_ttl = Some(Duration::from_secs(120));
    assert_eq!(settings.sweep_interval(), Duration::from_secs(30));
}
