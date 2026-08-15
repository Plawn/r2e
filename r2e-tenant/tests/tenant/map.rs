//! `Tenanted<T>`: single flight, failure handling, negative caching, timeouts,
//! eviction and drain.
//!
//! These are the invariants documented on the type. Each one exists because the
//! naive implementation gets it wrong: a `DashMap<TenantId, T>` with
//! `entry().or_insert_with(create)` would create N times under load, cache the
//! first failure forever, let a hanging `create` park every waiter, and leak a
//! slot per made-up tenant id.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_core::http::StatusCode;
use r2e_tenant::{TenantError, TenantId, Tenanted, TenantedSettings};

use crate::fixtures::{map_with, tid, wait_for, Behaviour, Gate, Resource, ScriptedSource};

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
async fn a_cascade_to_a_type_without_a_map_names_the_missing_plugin() {
    // The source asks the cascade for a type no `PerTenant` plugin provides a
    // map for: the failure must name the plugin to add, not panic or 503.
    #[derive(Clone, Debug)]
    struct Derived;

    #[derive(Clone)]
    struct Cascading;

    impl r2e_tenant::TenantSource<Derived> for Cascading {
        fn create<'a>(
            &'a self,
            _tenant: &'a TenantId,
            ctx: &'a r2e_tenant::TenantContext<'a>,
        ) -> r2e_tenant::BoxFuture<'a, Result<Option<Derived>, r2e_tenant::BoxError>> {
            Box::pin(async move {
                let _ = ctx.get::<Resource>().await?;
                Ok(Some(Derived))
            })
        }
    }

    let map: Tenanted<Derived> = Tenanted::new(
        Arc::new(Cascading),
        r2e_core::plugin::GraphHandle::default(),
        settings(),
        None,
    );

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
        r2e_core::plugin::GraphHandle::default(),
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

// ── removal racing creation ────────────────────────────────────────────────
//
// The rule under test: removal only ever takes a *ready* slot. Detaching an
// in-flight one would let the creation finish into a slot the map no longer
// holds — the caller gets a resource the map will never dispose of.

#[tokio::test]
async fn eviction_leaves_an_in_flight_creation_in_the_map() {
    let gate = Gate::new();
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Gated(gate.clone())),
        settings(),
    );
    let acme = tid("acme");

    let creating = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    gate.wait_started().await;

    assert!(
        !map.evict(&acme).await,
        "an in-flight creation has nothing to dispose of yet"
    );
    assert!(!map.invalidate(&acme), "neither does invalidate");
    assert_eq!(map.stats().len(), 1, "the slot stays mapped");

    gate.release();
    let resource = creating.await.unwrap().expect("the creation completes");
    assert_eq!(
        map.peek(&acme),
        Some(resource),
        "the value landed in the slot the map still holds"
    );

    // And it is disposed of exactly once, now that it is there to dispose of.
    assert!(map.evict(&acme).await);
    assert_eq!(source.disposals(), ["acme"]);
    assert_eq!(source.double_disposals(), 0);
    assert_eq!(map.metrics().disposed, 1);
}

#[tokio::test]
async fn drain_disposes_a_creation_that_lands_after_shutdown_started() {
    let gate = Gate::new();
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Gated(gate.clone())),
        settings(),
    );
    let acme = tid("acme");

    let creating = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    gate.wait_started().await;

    // Shutdown starts while the pool is still being opened. `drain` counts the
    // resolution as in-flight work, so it does not return while a value it will
    // have to close is still being built.
    let draining = tokio::spawn({
        let map = map.clone();
        async move { map.drain().await }
    });
    settle().await;
    assert!(
        !draining.is_finished(),
        "drain waits for the creation it will have to dispose of"
    );
    gate.release();
    draining.await.expect("the drain finishes once the creation lands");
    assert_eq!(
        source.disposals(),
        ["acme"],
        "and the value was closed before drain returned"
    );

    let err = creating
        .await
        .unwrap()
        .expect_err("a resource created during shutdown is not handed out");
    assert!(matches!(err, TenantError::Unavailable { .. }), "{err:?}");
    assert!(err.to_string().contains("draining"), "{err}");
    assert_eq!(
        source.disposals(),
        ["acme"],
        "what was built during the drain is still disposed of"
    );
    assert_eq!(source.double_disposals(), 0);
    assert!(map.active().is_empty(), "nothing leaked into the map");
}

#[tokio::test]
async fn draining_rejects_new_resolutions_instead_of_repopulating() {
    let (map, source) = map_with(ScriptedSource::new(), settings());
    map.get(&tid("acme")).await.unwrap();

    map.drain().await;

    let err = map.get(&tid("globex")).await.unwrap_err();
    assert!(matches!(err, TenantError::Unavailable { .. }), "{err:?}");
    assert_eq!(
        source.creates(),
        1,
        "a request arriving mid-shutdown must not open a new resource"
    );
    assert!(map.active().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_evict_and_drain_dispose_each_value_once() {
    let tenants = ["a", "b", "c", "d", "e"];
    // A slow dispose puts both removals inside `dispose` at the same time,
    // which is the interleaving the per-slot gate exists for.
    let (map, source) = map_with(
        ScriptedSource::with_dispose_delay(Behaviour::Ok, Duration::from_millis(20)),
        settings(),
    );
    for tenant in tenants {
        map.get(&tid(tenant)).await.unwrap();
    }

    let evictions: Vec<_> = tenants
        .iter()
        .map(|tenant| {
            let (map, tenant) = (map.clone(), tid(tenant));
            tokio::spawn(async move { map.evict(&tenant).await })
        })
        .collect();
    let draining = tokio::spawn({
        let map = map.clone();
        async move { map.drain().await }
    });

    let mut evicted = 0;
    for eviction in evictions {
        if eviction.await.unwrap() {
            evicted += 1;
        }
    }
    draining.await.unwrap();

    assert_eq!(
        source.sorted_disposals(),
        tenants,
        "every value is disposed of, exactly once"
    );
    assert_eq!(source.double_disposals(), 0);
    assert_eq!(map.metrics().disposed, tenants.len() as u64);
    assert!(map.active().is_empty());
    assert!(
        evicted <= tenants.len(),
        "each eviction reports whether it was the one that removed the slot"
    );
}

#[tokio::test]
async fn invalidate_disposes_in_the_background() {
    let (map, source) = map_with(ScriptedSource::new(), settings());
    let acme = tid("acme");
    map.get(&acme).await.unwrap();

    assert!(map.invalidate(&acme));
    assert!(
        map.peek(&acme).is_none(),
        "the resource is gone from the map straight away"
    );
    wait_for("invalidate to dispose", || source.disposals() == ["acme"]).await;
    assert_eq!(map.metrics().disposed, 1);
    assert_eq!(source.double_disposals(), 0);
}

// ── panics and cancellation ────────────────────────────────────────────────

#[tokio::test]
async fn a_panicking_create_leaves_no_slot_behind() {
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Panic),
        settings(),
    );
    let hostile = tid("hostile");

    let panicked = tokio::spawn({
        let (map, hostile) = (map.clone(), hostile.clone());
        async move { map.get(&hostile).await }
    })
    .await;
    assert!(panicked.unwrap_err().is_panic(), "the panic propagates");
    assert!(
        map.stats().is_empty(),
        "a panicking source must not accumulate empty slots"
    );

    // The tenant is not poisoned: once the source stops panicking it works.
    source.set("hostile", Behaviour::Ok);
    assert_eq!(map.get(&hostile).await.unwrap().tenant, "hostile");
}

#[tokio::test]
async fn a_cancelled_creation_leaves_no_slot_behind() {
    let gate = Gate::new();
    let (map, _source) = map_with(
        ScriptedSource::with_default(Behaviour::Gated(gate.clone())),
        settings(),
    );
    let acme = tid("acme");

    let creating = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    gate.wait_started().await;

    // The client hung up: the whole resolve future is dropped mid-`create`.
    creating.abort();
    assert!(creating.await.unwrap_err().is_cancelled());
    assert!(
        map.stats().is_empty(),
        "a cancelled creation must not leave an empty slot mapped"
    );

    gate.release();
    assert_eq!(
        map.get(&acme).await.expect("a later request works").tenant,
        "acme"
    );
}

// ── waiters, detached cells and self-healing ───────────────────────────────
//
// `tokio::sync::OnceCell` lets waiters take a failed (or cancelled)
// initialization over in turn. That means a cell can outlive its slot: the
// attempt that owned it cleaned the slot out of the map, and the waiter that
// inherits it succeeds into a cell nobody is mapped to. Two rules keep values
// from leaking there: only the task actually *running* an initializer arms the
// cleanup, and a success re-attaches its slot.

/// Let a just-spawned task run until it parks.
///
/// The waiters below park inside `OnceCell::get_or_try_init`, which is not
/// observable from the outside. Yielding is what actually hands them the
/// executor — `resolve` runs straight through to that await with nothing to
/// block on — so this is a scheduling turn, not a timing guess; the callers
/// then assert an observable consequence (`creates()`, `is_finished()`) rather
/// than trusting the wait. The trailing tick covers a multi-threaded flavour.
async fn settle() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(1)).await;
}

/// A waiter runs no initializer, so cancelling it must leave the map alone.
/// Detaching there would let the real creation finish into a slot the map no
/// longer holds — the caller gets a value nothing will ever dispose of.
#[tokio::test]
async fn a_cancelled_waiter_leaves_the_running_creation_mapped() {
    let gate = Gate::new();
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Gated(gate.clone())),
        settings(),
    );
    let acme = tid("acme");

    let creating = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    gate.wait_started().await;

    // A second request for the same cold tenant: single flight parks it on the
    // cell instead of starting a creation of its own.
    let waiter = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    settle().await;
    assert_eq!(source.creates(), 1, "the waiter must not start a creation");
    assert!(
        !waiter.is_finished(),
        "the waiter must still be parked on the cell when it is cancelled — \
         a cancellation that lands after it returned proves nothing"
    );

    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert_eq!(
        map.stats().len(),
        1,
        "a cancelled waiter must not detach the creation it was waiting for"
    );

    gate.release();
    let resource = creating.await.unwrap().expect("the creation completes");
    assert_eq!(
        map.peek(&acme),
        Some(resource),
        "the value landed in the slot the map still holds"
    );

    // And it is the map's to dispose of — exactly once.
    assert!(map.evict(&acme).await);
    assert_eq!(source.disposals(), ["acme"]);
    assert_eq!(source.double_disposals(), 0);
    assert_eq!(map.metrics().disposed, 1);
}

/// A failed initialization releases its waiters, and the failing caller removes
/// the slot. The waiter that retries succeeds into that detached cell — and has
/// to put the slot back, or its value is the map's in nobody's book.
#[tokio::test]
async fn a_waiter_that_retries_after_a_failure_owns_its_value() {
    let gate = Gate::new();
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::GatedFail(gate.clone())),
        settings(),
    );
    let acme = tid("acme");

    let failing = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    gate.wait_started().await;
    let waiter = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    settle().await;
    assert_eq!(source.creates(), 1, "the waiter must not start a creation");
    assert!(
        !waiter.is_finished(),
        "the waiter must still be parked when the first attempt fails"
    );

    // The tenant becomes healthy just as the first attempt gives up.
    source.set("acme", Behaviour::Ok);
    gate.release();

    let err = failing.await.unwrap().expect_err("the first attempt failed");
    assert!(matches!(err, TenantError::Unavailable { .. }), "{err:?}");
    let resource = waiter
        .await
        .unwrap()
        .expect("the queued waiter retries the initializer and succeeds");

    assert_eq!(source.creates(), 2, "one attempt each, no wave");
    assert_eq!(
        map.peek(&acme),
        Some(resource),
        "the retry's value must be the one the map owns, not a detached cell's"
    );
    assert!(map.evict(&acme).await);
    assert_eq!(source.disposals(), ["acme"]);
    assert_eq!(source.double_disposals(), 0);
    assert_eq!(map.metrics().disposed, 1);
}

/// The other half: the cell is detached by a *cancelled* initializer, and the
/// waiter that inherits it succeeds. Same rule, different detacher.
#[tokio::test]
async fn a_waiter_reattaches_the_slot_a_cancelled_creation_detached() {
    let gate = Gate::new();
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Gated(gate.clone())),
        settings(),
    );
    let acme = tid("acme");

    let creating = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    gate.wait_started().await;
    let waiter = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    settle().await;
    assert!(
        !waiter.is_finished(),
        "the waiter must still be parked on the cell it is about to inherit"
    );

    // The client running the creation hangs up: its guard cleans the slot out.
    creating.abort();
    assert!(creating.await.unwrap_err().is_cancelled());
    assert!(
        map.stats().is_empty(),
        "the cancelled initializer detaches its own slot"
    );

    gate.release();
    let resource = waiter
        .await
        .unwrap()
        .expect("the waiter takes the initializer over and succeeds");
    assert_eq!(
        map.peek(&acme),
        Some(resource),
        "a success on a detached cell must re-attach its slot"
    );

    // Nothing leaked: the value reaches `dispose` at shutdown.
    map.drain().await;
    assert_eq!(source.disposals(), ["acme"]);
    assert_eq!(source.double_disposals(), 0);
}

/// And when a *fresh* resolve recreated the key while the detached cell was
/// still creating, the map keeps the fresh slot: the orphaned value is disposed
/// of in the background and still handed to its caller (there are no leases).
#[tokio::test]
async fn an_orphaned_creation_is_disposed_and_nothing_leaks() {
    let gate = Gate::new();
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Gated(gate.clone())),
        settings(),
    );
    let acme = tid("acme");

    let creating = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    gate.wait_started().await;
    let waiter = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    settle().await;

    creating.abort();
    assert!(creating.await.unwrap_err().is_cancelled());
    assert!(map.stats().is_empty(), "the slot is detached");

    // A brand-new request arrives while the waiter is still creating in the
    // detached cell: it installs a *different* slot under the same key.
    let fresh = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    wait_for("the fresh resolve to install its slot", || {
        !map.stats().is_empty()
    })
    .await;

    gate.release();
    let orphan = waiter.await.unwrap().expect("the waiter still succeeds");
    let kept = fresh.await.unwrap().expect("so does the fresh resolve");
    assert_ne!(orphan, kept, "two distinct resources were built");
    assert_eq!(
        map.peek(&acme),
        Some(kept),
        "the slot the map holds is the fresh one"
    );

    wait_for("the orphaned value to be disposed", || {
        map.metrics().disposed == 1
    })
    .await;
    map.drain().await;
    assert_eq!(
        source.disposals().len(),
        2,
        "every value that was built reaches dispose: {:?}",
        source.disposals()
    );
    assert_eq!(map.metrics().created, 2);
    assert_eq!(map.metrics().disposed, 2);
    assert_eq!(source.double_disposals(), 0);
    assert!(map.active().is_empty(), "nothing leaked into the map");
}

/// The second of the two rules, on its own: with a negative entry and a ready
/// slot for the same tenant, the slot wins.
///
/// This state is what the *first* rule (only the key's owner may negative-cache)
/// exists to prevent, so it is reached through the test seam rather than through
/// a race — otherwise the two rules mask each other and neither is pinned. This
/// is the defence that keeps a live tenant serving if the first one ever
/// regresses: without it the tenant is 404 (or silently falls back) for a whole
/// `negative-ttl` while its pool sits right there in the map.
#[tokio::test]
async fn a_ready_slot_wins_over_a_negative_entry() {
    let settings = TenantedSettings {
        negative_ttl: Some(Duration::from_secs(30)),
        ..settings()
    };
    let (map, source) = map_with(ScriptedSource::new(), settings);
    let acme = tid("acme");

    let live = map.get(&acme).await.expect("the tenant is provisioned");
    map.force_negative_entry(&acme);
    assert_eq!(map.metrics().negative, 1, "the memo is in place");

    assert_eq!(
        map.get(&acme).await.expect("the tenant is still live"),
        live,
        "a cached resource is never shadowed by a negative memo"
    );
    assert_eq!(source.creates(), 1, "that was a cache hit, not a re-creation");
    assert_eq!(
        map.metrics().negative,
        0,
        "and the stale memo is dropped on the way out"
    );
}

/// The first rule, on its own: a *detached* attempt that reports the tenant
/// unknown after a fresh resolve cached a real value writes no negative entry.
///
/// It speaks for nobody — another slot already answers for this key. Asserted at
/// the instant the detached attempt completes, before any resolve that would
/// clean the memo up, so the ready-first rule cannot mask a regression here.
#[tokio::test]
async fn a_detached_unknown_verdict_is_never_remembered() {
    let first = Gate::new();
    let unknown = Gate::new();
    // A negative TTL long enough that only a *rule* — not an expiry — can clear
    // the memo this test is about.
    let settings = TenantedSettings {
        negative_ttl: Some(Duration::from_secs(30)),
        ..settings()
    };
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Gated(first.clone())),
        settings,
    );
    let acme = tid("acme");

    // A creation parks in the source; a second request queues on its cell.
    let creating = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    first.wait_started().await;
    let waiter = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    settle().await;
    assert!(!waiter.is_finished(), "the waiter is parked on the cell");

    // The directory is about to answer "not provisioned" — but slowly, held at
    // a gate — and the first client hangs up, detaching the slot. The waiter
    // inherits the initializer and walks into that verdict.
    source.set("acme", Behaviour::GatedUnknown(unknown.clone()));
    creating.abort();
    assert!(creating.await.unwrap_err().is_cancelled());
    unknown.wait_started().await;
    assert!(map.stats().is_empty(), "the cell is detached from the map");

    // Meanwhile the tenant *is* provisioned, and a fresh request builds it.
    source.set("acme", Behaviour::Ok);
    let live = map.get(&acme).await.expect("the fresh resolve succeeds");
    assert_eq!(map.peek(&acme), Some(live.clone()));

    // Now the stale verdict lands. Its own caller is told "unknown" — that is
    // what its attempt saw — but it must not memoize it for anyone else.
    unknown.release();
    let err = waiter
        .await
        .unwrap()
        .expect_err("that attempt's own answer is still `unknown`");
    assert!(matches!(err, TenantError::Unknown(_)), "{err:?}");

    // Immediately, and before any resolve: a `get` here would clean the memo up
    // on its way past and hide the very thing being asserted.
    assert_eq!(
        map.metrics().negative,
        0,
        "a detached attempt must not negative-cache over a live slot"
    );
    assert_eq!(
        map.peek(&acme),
        Some(live),
        "and the live slot is untouched by it"
    );

    assert!(map.evict(&acme).await);
    assert_eq!(source.disposals(), ["acme"]);
    assert_eq!(source.double_disposals(), 0);
}

/// A stale "unknown" verdict must never abort a *fresh creation already in
/// flight* under the same key.
///
/// The dangerous window is inside `remember_negative_owned`: it tests who owns
/// the key and then writes the memo. If those are two critical sections, a fresh
/// resolve can install its slot in between and then be turned away by that memo
/// at the negative re-check *inside its own initializer* — the source never
/// asked, the caller told "unknown" about a tenant that exists. Here the fresh
/// slot is mapped (and still building) before the stale verdict lands, so the
/// ownership test must see it and write nothing.
#[tokio::test]
async fn a_stale_verdict_never_aborts_a_fresh_creation_in_flight() {
    let first = Gate::new();
    let unknown = Gate::new();
    let fresh = Gate::new();
    let settings = TenantedSettings {
        negative_ttl: Some(Duration::from_secs(30)),
        ..settings()
    };
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Gated(first.clone())),
        settings,
    );
    let acme = tid("acme");

    // Detach a cell: a creation parks, a waiter queues behind it, the creation's
    // client hangs up, and the waiter inherits the initializer — walking into a
    // directory that answers "not provisioned", slowly.
    let creating = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    first.wait_started().await;
    let stale = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    settle().await;
    assert!(!stale.is_finished(), "the waiter is parked on the cell");

    source.set("acme", Behaviour::GatedUnknown(unknown.clone()));
    creating.abort();
    assert!(creating.await.unwrap_err().is_cancelled());
    unknown.wait_started().await;
    assert!(map.stats().is_empty(), "the cell is detached from the map");

    // The tenant *is* provisioned, and a fresh request starts building it. Its
    // slot is in the map and still empty when the stale verdict lands.
    source.set("acme", Behaviour::Gated(fresh.clone()));
    let live = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    fresh.wait_started().await;
    assert_eq!(map.stats().len(), 1, "the fresh slot is mapped, still building");

    // Now the stale verdict lands. Its own caller hears "unknown"; nobody else
    // may.
    unknown.release();
    let err = stale
        .await
        .unwrap()
        .expect_err("that attempt's own answer is still `unknown`");
    assert!(matches!(err, TenantError::Unknown(_)), "{err:?}");
    assert_eq!(
        map.metrics().negative,
        0,
        "a detached attempt must not memoize over someone else's in-flight slot"
    );

    // And the fresh creation runs to completion off the source, un-aborted.
    fresh.release();
    let value = live.await.unwrap().expect("the fresh creation succeeds");
    assert_eq!(map.peek(&acme), Some(value));
    assert_eq!(
        map.metrics().created,
        1,
        "the fresh initializer reached the source and cached its value"
    );
    assert_eq!(
        source.creates(),
        3,
        "one aborted, one unknown, one real — nothing was short-circuited"
    );

    assert!(map.evict(&acme).await);
    assert_eq!(source.disposals(), ["acme"]);
    assert_eq!(source.double_disposals(), 0);
}

/// The two-participant race, forced in both orders: whoever classifies the
/// shared value as orphaned commits its disposal gate **inline**, under the
/// key's shard guard, so the other participant can never restore a dying value.
///
/// The dangerous shape is real and does not need a public removal: a competing
/// empty slot appears under the key (one participant orphans against it) and
/// then vanishes when its own initializer fails — a cleanup that deliberately
/// does not bump the epoch, or a waiter inheriting a cancelled cell could never
/// legitimately reattach. The next participant then sees a vacant key at a
/// matching epoch: textbook "restore". Only the gate, committed in the same
/// critical section as the classification, tells it not to.
#[tokio::test]
async fn an_orphaned_value_is_never_restored_by_the_other_participant() {
    let (map, source) = map_with(ScriptedSource::new(), settings());
    let acme = tid("acme");
    map.get(&acme).await.expect("the tenant is built");

    let (owed, refused) = map.force_reattach_race(&acme, true).await;
    assert!(owed, "the orphaning participant owns the disposal");
    assert!(
        refused,
        "and the other one must refuse to put the dying value back"
    );
    assert_eq!(
        map.peek(&acme),
        None,
        "the key stays empty rather than caching a closed resource"
    );
    assert_eq!(source.disposals(), ["acme"]);
    assert_eq!(source.double_disposals(), 0);
}

/// The same race the other way round: the restore lands first, so the
/// would-be orphaner finds the key holding *its own* slot — `ptr_eq` → `Kept` —
/// and takes no gate, spawns no disposal.
///
/// This is the half that keeps the fix from being a leak: committing the gate on
/// every classification, rather than only when the key is somebody else's, would
/// close a value the map is still handing out.
#[tokio::test]
async fn a_restored_value_is_kept_by_the_other_participant() {
    let (map, source) = map_with(ScriptedSource::new(), settings());
    let acme = tid("acme");
    let value = map.get(&acme).await.expect("the tenant is built");

    let (restored, kept) = map.force_reattach_race(&acme, false).await;
    assert!(restored, "the vacant key at a matching epoch is refilled");
    assert!(kept, "and the other participant recognises its own slot");
    assert_eq!(
        map.peek(&acme),
        Some(value),
        "the value is the map's again, alive"
    );
    assert!(
        source.disposals().is_empty(),
        "nothing was disposed of: {:?}",
        source.disposals()
    );

    // And it is still an ordinary cached resource afterwards.
    assert!(map.evict(&acme).await);
    assert_eq!(source.disposals(), ["acme"]);
    assert_eq!(source.double_disposals(), 0);
}

/// An awaited removal keeps its own disposal: a participant arriving in the
/// window right after the slot came out of the map cannot take the closing over
/// onto a detached task.
///
/// The window is real — the removal bumps the epoch before it takes the shard
/// lock, so a late participant sees a vacant key at a mismatched epoch and
/// classifies the value `Orphaned`, which is a gate commit. If the remover has
/// not committed by then, the participant wins, spawns the disposal, and
/// `evict().await` returns before the pool is closed — worse, onto a task a
/// shutting-down runtime may never poll. Committing inside `take_ready`'s
/// critical section is what settles it.
#[tokio::test]
async fn an_awaited_removal_closes_the_value_before_it_returns() {
    let (map, source) = map_with(ScriptedSource::new(), settings());
    let acme = tid("acme");
    map.get(&acme).await.expect("the tenant is built");

    let (remover_owed, late_stood_down) = map.force_remove_race(&acme, true).await;
    assert!(
        remover_owed,
        "the remover took the gate under the lock, so the disposal is its own"
    );
    assert!(
        late_stood_down,
        "and the late participant lost the CAS: nothing detached"
    );
    assert_eq!(
        source.disposals(),
        ["acme"],
        "the value was closed by the time the removal returned"
    );
    assert_eq!(source.double_disposals(), 0);
    assert_eq!(map.peek(&acme), None);
}

/// The same race the other way round: the participant puts the slot back before
/// the removal looks, so the removal finds a mapped, ready slot — and removes
/// *and* commits it in one critical section, disposing of it itself.
#[tokio::test]
async fn a_removal_after_a_restore_disposes_of_the_restored_value() {
    let (map, source) = map_with(ScriptedSource::new(), settings());
    let acme = tid("acme");
    map.get(&acme).await.expect("the tenant is built");

    let (restored, remover_owed) = map.force_remove_race(&acme, false).await;
    assert!(restored, "the participant put its live value back");
    assert!(
        remover_owed,
        "and the removal that follows owns the disposal of what it removed"
    );
    assert_eq!(source.disposals(), ["acme"]);
    assert_eq!(source.double_disposals(), 0);
    assert_eq!(map.peek(&acme), None);
}

/// A drain that finds the key vacant still closes the value it is draining
/// before it returns.
///
/// This is the identity-conditional removal (`take_slot`) reaching the branch
/// where the key does *not* hold its slot — the slot was detached, so there is
/// nothing to unmap. The gate still has to be taken there, and taken **under the
/// key's entry guard**: a participant holding that same slot is one `reattach`
/// away from putting it back, and the epoch cannot stop it (a cancelled
/// initializer's cleanup does not bump). With the CAS outside the guard, the
/// restore lands between the vacant read and the CAS, the participant wins the
/// gate, and `drain` returns with the value still open on a detached task.
#[tokio::test]
async fn a_drain_on_a_detached_slot_closes_it_before_it_returns() {
    let (map, source) = map_with(ScriptedSource::new(), settings());
    let acme = tid("acme");
    map.get(&acme).await.expect("the tenant is built");

    let (drainer_owed, late_stood_down) = map.force_take_slot_race(&acme, false).await;
    assert!(
        drainer_owed,
        "the drainer took the gate under the vacant entry guard"
    );
    assert!(
        late_stood_down,
        "and the participant that arrived after it refused to restore, spawning nothing"
    );
    assert_eq!(
        source.disposals(),
        ["acme"],
        "the value was closed by the time the drain returned"
    );
    assert_eq!(source.double_disposals(), 0);
    assert_eq!(map.peek(&acme), None);
}

/// The same race the other way round: the participant restores the slot first,
/// so the drain finds the key occupied by that very slot — and removes *and*
/// commits it in one critical section rather than leaving a restored value
/// behind a returned `drain`.
#[tokio::test]
async fn a_drain_after_a_restore_closes_the_restored_value() {
    let (map, source) = map_with(ScriptedSource::new(), settings());
    let acme = tid("acme");
    map.get(&acme).await.expect("the tenant is built");

    let (drainer_owed, unmapped) = map.force_take_slot_race(&acme, true).await;
    assert!(
        drainer_owed,
        "the drain owns the disposal of the slot it found back under the key"
    );
    assert!(unmapped, "and it left the key empty");
    assert_eq!(source.disposals(), ["acme"]);
    assert_eq!(source.double_disposals(), 0);
    assert_eq!(map.peek(&acme), None);
}

/// `drain` does not return while a disposal somebody *else* committed is still
/// running.
///
/// Walking the map is not enough: `invalidate` unmaps the slot and hands the
/// close to a detached task, so by the time `drain` looks there is nothing to
/// see and nobody it can await. Without the in-flight counter it returns while
/// the pool is still closing — on a runtime that is itself shutting down and may
/// never poll that task again.
#[tokio::test]
async fn drain_waits_for_a_disposal_it_does_not_own() {
    let closing = Gate::new();
    let (map, source) = map_with(
        ScriptedSource::with_dispose_gate(Behaviour::Ok, closing.clone()),
        settings(),
    );
    let acme = tid("acme");
    map.get(&acme).await.expect("the tenant is built");

    assert!(map.invalidate(&acme), "the slot is unmapped right away");
    closing.wait_started().await;
    assert_eq!(map.peek(&acme), None, "drain has nothing left to walk");

    let draining = tokio::spawn({
        let map = map.clone();
        async move { map.drain().await }
    });
    settle().await;
    assert!(
        !draining.is_finished(),
        "drain waits for the close it can neither see nor own"
    );

    closing.release();
    draining
        .await
        .expect("the drain finishes once the close does");
    assert_eq!(
        source.disposals(),
        ["acme"],
        "the value was closed by the time drain returned"
    );
    assert_eq!(source.double_disposals(), 0);
}

/// A flood of requests arriving after the latch cannot hold shutdown open.
///
/// `resolve` has to count itself as in-flight work for `drain` to wait on, but
/// counting *before* reading the latch turns every rejected request into a
/// reason to keep waiting: the counter never passes through zero, the
/// notify-on-zero never fires, and shutdown never completes while traffic
/// continues. The listener is still accepting while the drain hook runs, so this
/// is an ordinary production shape. The double check — read the latch, then
/// count, then read it again — is what makes the counted set finite.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_flood_of_post_shutdown_requests_cannot_starve_the_drain() {
    let (map, source) = map_with(ScriptedSource::new(), settings());
    let acme = tid("acme");
    map.get(&acme).await.expect("the tenant is built");

    let stop = Arc::new(AtomicBool::new(false));
    let hot = Arc::new(AtomicUsize::new(0));
    let mut streams = Vec::new();
    for _ in 0..4 {
        let (map, acme, stop, hot) = (map.clone(), acme.clone(), stop.clone(), hot.clone());
        streams.push(tokio::spawn(async move {
            let mut rejected = 0usize;
            while !stop.load(Ordering::Relaxed) {
                if let Err(err) = map.get(&acme).await {
                    assert!(err.to_string().contains("draining"), "{err}");
                    rejected += 1;
                }
                let n = hot.fetch_add(1, Ordering::Relaxed);
                // The rejected path has no await point of its own, so without
                // yielding the streams would starve the runtime rather than the
                // drain — a different bug than the one under test. Rarely,
                // though: a yield per request would leave the counter at zero
                // most of the time, which is the very thing being tested.
                if n % 64 == 0 {
                    tokio::task::yield_now().await;
                }
            }
            rejected
        }));
    }
    // Every stream is spinning before the latch goes up: a drain that finishes
    // before the flood even starts would prove nothing.
    wait_for("the request flood to be running", || {
        hot.load(Ordering::Relaxed) > 2_000
    })
    .await;

    tokio::time::timeout(Duration::from_secs(5), map.drain())
        .await
        .expect("a flood of rejected requests must not hold shutdown open");

    stop.store(true, Ordering::Relaxed);
    let mut rejected = 0;
    for stream in streams {
        rejected += stream.await.expect("the stream task finishes");
    }
    assert!(
        rejected > 0,
        "the flood really did keep hitting the latched map"
    );
    assert_eq!(source.disposals(), ["acme"]);
    assert_eq!(source.double_disposals(), 0);
}

/// A slot whose disposal gate is already committed is never put back into the
/// map.
///
/// Reaching `reattach` with an already-disposed slot is an ordinary occurrence,
/// not a theoretical one: every removal and every orphaning now commits the gate
/// under the key's shard lock, so a participant arriving a moment later routinely
/// finds one. The seam below neutralises the epoch rule so that *only* the gate
/// can answer, which is the half this test is about — the cost of getting it
/// wrong is a closed pool cached as a live one.
#[tokio::test]
async fn a_disposed_slot_is_never_put_back() {
    let (map, source) = map_with(ScriptedSource::new(), settings());
    let acme = tid("acme");
    map.get(&acme).await.expect("the tenant is built");

    assert!(
        map.force_reattach_after_dispose(&acme).await,
        "a slot whose disposal gate is committed must be refused"
    );
    assert_eq!(
        map.peek(&acme),
        None,
        "and the key stays empty rather than holding a closed resource"
    );
    assert_eq!(source.disposals(), ["acme"]);
    assert_eq!(source.double_disposals(), 0);
}

/// A detached creation that started *before* an `invalidate` must not put its
/// value back afterwards.
///
/// A vacant key reads the same whether nobody ever mapped the tenant or an
/// `invalidate` just emptied it, so the reattach cannot go on vacancy alone: it
/// would resurrect precisely the resource the caller asked to kill, and leave it
/// there until the next invalidation. The map-wide epoch — bumped by every
/// removal, read before the creation starts — is what tells the two apart.
#[tokio::test]
async fn an_invalidate_fences_a_creation_that_started_before_it() {
    let first = Gate::new();
    let stale = Gate::new();
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Gated(first.clone())),
        settings(),
    );
    let acme = tid("acme");

    // Detach a cell: a creation parks, a waiter queues behind it, the creation's
    // client hangs up. The waiter inherits the cell with no slot in the map.
    let creating = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    first.wait_started().await;
    let detached = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    settle().await;
    source.set("acme", Behaviour::Gated(stale.clone()));
    creating.abort();
    assert!(creating.await.unwrap_err().is_cancelled());
    stale.wait_started().await;
    assert!(map.stats().is_empty(), "the cell is detached from the map");

    // A fresh request builds and caches a resource, and the operator then
    // rotates the tenant: `invalidate` removes it.
    source.set("acme", Behaviour::Ok);
    let rotated = map.get(&acme).await.expect("the fresh resolve succeeds");
    assert!(map.invalidate(&acme), "the rotation removes the live slot");

    // The pre-rotation creation lands now. Its caller still gets a value (there
    // are no leases) but the map must not take it.
    stale.release();
    let orphan = detached.await.unwrap().expect("its caller is still served");
    assert_ne!(orphan, rotated, "two distinct resources were built");
    assert_eq!(
        map.peek(&acme),
        None,
        "a value created before the invalidate must not come back after it"
    );

    // And the next request rebuilds from the source, as the rotation intended.
    let fresh = map.get(&acme).await.expect("the tenant is rebuilt");
    assert_ne!(fresh, orphan);
    assert_ne!(fresh, rotated);
    assert_eq!(map.metrics().created, 3);

    // Nothing leaked: the rotated value and the fenced orphan both reach dispose.
    wait_for("the fenced orphan to be disposed", || {
        map.metrics().disposed == 2
    })
    .await;
    map.drain().await;
    assert_eq!(source.disposals().len(), 3);
    assert_eq!(source.double_disposals(), 0);
}

/// The same fence on the negative side: an `Ok(None)` from before an
/// `invalidate` must not repopulate the negative cache the `invalidate` cleared.
#[tokio::test]
async fn an_invalidate_fences_an_unknown_verdict_from_before_it() {
    let first = Gate::new();
    let unknown = Gate::new();
    let settings = TenantedSettings {
        negative_ttl: Some(Duration::from_secs(30)),
        ..settings()
    };
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Gated(first.clone())),
        settings,
    );
    let acme = tid("acme");

    let creating = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    first.wait_started().await;
    let detached = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    settle().await;
    source.set("acme", Behaviour::GatedUnknown(unknown.clone()));
    creating.abort();
    assert!(creating.await.unwrap_err().is_cancelled());
    unknown.wait_started().await;

    source.set("acme", Behaviour::Ok);
    map.get(&acme).await.expect("the fresh resolve succeeds");
    assert!(map.invalidate(&acme), "the rotation removes the live slot");

    unknown.release();
    let err = detached
        .await
        .unwrap()
        .expect_err("that attempt's own answer is still `unknown`");
    assert!(matches!(err, TenantError::Unknown(_)), "{err:?}");
    assert_eq!(
        map.metrics().negative,
        0,
        "an unknown verdict from before the invalidate must not be remembered \
         after it — the tenant would be 404 for a whole negative-ttl"
    );

    // Proof it is not just the memo being cleaned on the way past: the next
    // request reaches the source again.
    map.get(&acme).await.expect("the tenant resolves again");
    assert_eq!(source.creates(), 4);
}

/// Draining wins over the reattach: a detached creation landing behind the latch
/// disposes of what it built instead of putting the slot back.
#[tokio::test]
async fn a_detached_creation_landing_during_a_drain_is_disposed_not_reattached() {
    let gate = Gate::new();
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Gated(gate.clone())),
        settings(),
    );
    let acme = tid("acme");

    let creating = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    gate.wait_started().await;
    let waiter = tokio::spawn({
        let (map, acme) = (map.clone(), acme.clone());
        async move { map.get(&acme).await }
    });
    settle().await;

    creating.abort();
    assert!(creating.await.unwrap_err().is_cancelled());

    // Shutdown starts while the waiter is creating in the *detached* cell — the
    // one shape a walk of the map cannot see. The waiter is counted in-flight,
    // so `drain` waits for it, and for the orphan disposal it spawns.
    let draining = tokio::spawn({
        let map = map.clone();
        async move { map.drain().await }
    });
    settle().await;
    assert!(
        !draining.is_finished(),
        "drain waits for the detached creation it cannot see"
    );
    gate.release();
    draining
        .await
        .expect("the drain finishes once the detached creation settles");
    assert_eq!(
        source.disposals(),
        ["acme"],
        "the orphan disposal completed before drain returned"
    );

    let err = waiter
        .await
        .unwrap()
        .expect_err("a resource created during shutdown is not handed out");
    assert!(matches!(err, TenantError::Unavailable { .. }), "{err:?}");
    assert!(err.to_string().contains("draining"), "{err}");
    assert_eq!(
        source.disposals(),
        ["acme"],
        "what was built during the drain is still disposed of"
    );
    assert_eq!(source.double_disposals(), 0);
    assert!(
        map.stats().is_empty(),
        "the drain latch must not be repopulated by a re-attach"
    );
}

// ── failure waves ──────────────────────────────────────────────────────────

#[tokio::test]
async fn an_unknown_cold_wave_asks_the_directory_once() {
    let mut settings = settings();
    settings.negative_ttl = Some(Duration::from_secs(30));
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::SlowUnknown(Duration::from_millis(30))),
        settings,
    );
    let ghost = tid("ghost");

    // 50 requests for one unknown tenant, all arriving before the first answer.
    let mut tasks = Vec::new();
    for _ in 0..50 {
        let (map, ghost) = (map.clone(), ghost.clone());
        tasks.push(tokio::spawn(async move { map.get(&ghost).await }));
    }
    for task in tasks {
        let err = task.await.unwrap().unwrap_err();
        assert!(matches!(err, TenantError::Unknown(_)), "{err:?}");
    }

    assert_eq!(
        source.creates(),
        1,
        "waiters that run the initializer in turn must see the negative entry \
         the first attempt wrote, not hammer the directory"
    );
    assert_eq!(map.metrics().negative, 1);
    assert!(map.stats().is_empty(), "no slot survives an unknown tenant");
}

// ── bounded caches ─────────────────────────────────────────────────────────

#[tokio::test]
async fn the_negative_cache_never_stays_over_its_bound() {
    let mut settings = settings();
    settings.negative_ttl = Some(Duration::from_secs(30));
    settings.max_negative = 8;
    let (map, _source) = map_with(ScriptedSource::with_default(Behaviour::Unknown), settings);

    for n in 0..100 {
        let ghost = tid(&format!("ghost-{n}"));
        assert!(map.get(&ghost).await.is_err());
        assert!(
            map.metrics().negative <= 8,
            "the negative cache went over max-negative at insert {n}: {}",
            map.metrics().negative
        );
    }
    assert_eq!(map.metrics().negative, 8, "and it stays full, not empty");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_concurrent_unknown_flood_stays_bounded() {
    let mut settings = settings();
    settings.negative_ttl = Some(Duration::from_secs(30));
    settings.max_negative = 8;
    let (map, _source) = map_with(ScriptedSource::with_default(Behaviour::Unknown), settings);

    let mut tasks = Vec::new();
    for worker in 0..16 {
        let map = map.clone();
        tasks.push(tokio::spawn(async move {
            for n in 0..25 {
                let ghost = tid(&format!("ghost-{worker}-{n}"));
                assert!(map.get(&ghost).await.is_err());
                // Concurrent inserts can be over the bound for a moment — each
                // of them trims — but never unboundedly so.
                let negative = map.metrics().negative;
                assert!(
                    negative <= 8 + 32,
                    "the negative cache is not bounded under concurrency: {negative}"
                );
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }

    assert!(
        map.metrics().negative <= 8,
        "once the flood settles the bound holds exactly: {}",
        map.metrics().negative
    );
}

#[tokio::test]
async fn max_active_is_enforced_without_a_sweep() {
    let mut settings = settings();
    settings.max_active = 2;
    let (map, source) = map_with(ScriptedSource::new(), settings);

    // No `sweep()` anywhere: the trim `enforce_max_active` kicks off after each
    // creation is what has to bring the map back under the cap.
    for tenant in ["first", "second", "third", "fourth"] {
        map.get(&tid(tenant)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    wait_for("the background trim to reach max-active", || {
        map.active().len() <= 2
    })
    .await;

    let active: Vec<String> = map
        .active()
        .iter()
        .map(|id| id.as_str().to_string())
        .collect();
    assert_eq!(active.len(), 2, "the cap must hold: {active:?}");
    assert!(
        !active.contains(&"first".to_string()) && !active.contains(&"second".to_string()),
        "the least recently used tenants are the victims: {active:?}"
    );
    assert!(
        source.disposals().contains(&"first".to_string()),
        "trimming disposes: {:?}",
        source.disposals()
    );
    assert_eq!(source.double_disposals(), 0);
}

#[tokio::test]
async fn a_cold_burst_settles_back_under_max_active() {
    let mut settings = settings();
    settings.max_active = 4;
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Slow(Duration::from_millis(10))),
        settings,
    );

    let mut tasks = Vec::new();
    for n in 0..40 {
        let map = map.clone();
        tasks.push(tokio::spawn(async move {
            map.get(&tid(&format!("tenant-{n}"))).await
        }));
    }
    for task in tasks {
        task.await.unwrap().expect("every cold tenant is served");
    }

    // `max-active` is a soft cap: the burst creates 40, the trim brings it back.
    wait_for("the cold burst to settle under max-active", || {
        map.active().len() <= 4
    })
    .await;
    assert_eq!(source.double_disposals(), 0);
    assert_eq!(
        source.disposals().len(),
        36,
        "everything trimmed is disposed of: {:?}",
        map.metrics()
    );
}

/// Every creation of a wave completes in the same scheduler burst: one of them
/// starts the trim, and all the others find the flag already set and decline to
/// schedule one. Whoever is trimming therefore has to re-check *after* clearing
/// the flag — and re-check the **ready** count, since the slots that made it
/// over the cap became ready while the trim was running.
///
/// The handoff window is a few sync instructions wide, so this is a convergence
/// test rather than a deterministic reproduction: the point is that no sweep and
/// no further request rescue the map. `wait_for` is the assertion — a lost
/// handoff leaves the map over the cap forever and times out here.
#[tokio::test]
async fn a_simultaneous_wave_of_completions_still_converges() {
    let gate = Gate::new();
    let mut settings = settings();
    settings.max_active = 2;
    let (map, source) = map_with(
        ScriptedSource::with_default(Behaviour::Gated(gate.clone())),
        settings,
    );

    let mut tasks = Vec::new();
    for n in 0..12 {
        let map = map.clone();
        tasks.push(tokio::spawn(async move {
            map.get(&tid(&format!("tenant-{n}"))).await
        }));
    }
    // All twelve are inside `create`, parked: none of them has trimmed yet.
    for _ in 0..12 {
        gate.wait_started().await;
    }
    assert_eq!(map.active().len(), 0, "nothing is ready before the release");

    gate.release();
    for task in tasks {
        task.await.unwrap().expect("every tenant is served");
    }

    wait_for("the wave to settle back under max-active", || {
        map.active().len() <= 2
    })
    .await;
    wait_for("every trimmed resource to be disposed of", || {
        source.disposals().len() == 10
    })
    .await;
    assert_eq!(map.active().len(), 2, "the cap holds: {:?}", map.active());
    assert_eq!(source.double_disposals(), 0);
    assert_eq!(map.metrics().created, 12);
}

#[tokio::test]
#[should_panic(expected = "max-active")]
async fn a_max_active_of_zero_is_rejected_at_wiring_time() {
    let mut settings = settings();
    settings.max_active = 0;
    let _ = map_with(ScriptedSource::new(), settings);
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
