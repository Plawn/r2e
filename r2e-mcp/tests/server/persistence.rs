//! HTTP session persistence (`McpSessionStore`, `mcp.session.store-ttl-secs`,
//! `McpServer::with_session_store` / `with_session_store_ttl`): a session's
//! handshake is written at `initialize` and re-armed at half-life, and a
//! request naming a session this process does not hold — another instance,
//! an idle eviction — rebuilds it from the record, running `session_init`
//! again with `SessionInit::is_restored()`.
//!
//! "Another instance" = a second app built on the same store. The clock is
//! virtual (`start_paused`): the record TTL, the idle timer and the half-life
//! all run on `r2e_core::rt::Instant`.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e_core::http::{Body, Request, Routes, StatusCode};
use r2e_core::rt::sync::Notify;
use r2e_core::{rt, AppBuilder, R2eConfig};
use r2e_mcp::{
    AppBuilderMcpExt, InMemoryMcpSessionStore, McpError, McpServer, McpSessionInit,
    McpSessionStore, McpSessionStoreFuture, McpSessions, SessionInit, SessionToolset,
};
use serde_json::json;
use tower::ServiceExt;

use crate::fixtures::{Calc, CallLog, FixtureTools};
use crate::support;

// ── Fixtures ───────────────────────────────────────────────────────────────

/// Records every `session_init` run: `(restored, session id known)`; can
/// fail the next run on demand (a handshake rmcp answers with an error).
#[derive(Clone, Default)]
struct InitProbe {
    runs: Arc<Mutex<Vec<(bool, bool)>>>,
    fail_next: Arc<AtomicBool>,
}

impl InitProbe {
    fn runs(&self) -> Vec<(bool, bool)> {
        self.runs.lock().unwrap().clone()
    }
}

impl McpSessionInit for InitProbe {
    async fn init(&self, session: &SessionInit) -> Result<SessionToolset, McpError> {
        self.runs
            .lock()
            .unwrap()
            .push((session.is_restored(), session.session_id().is_some()));
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(McpError::Internal("init hook down".into()));
        }
        Ok(SessionToolset::new())
    }
}

/// An in-memory store that logs its operations, can fail on demand and can
/// hold a write (a re-arm "in flight") or a read (a restore "in flight")
/// until released.
#[derive(Clone, Default)]
struct Recording {
    inner: InMemoryMcpSessionStore,
    ops: Arc<Mutex<Vec<String>>>,
    fail_load: Arc<AtomicBool>,
    fail_store: Arc<AtomicBool>,
    fail_delete: Arc<AtomicBool>,
    hold_writes: Arc<AtomicBool>,
    held: Arc<AtomicUsize>,
    release: Arc<Notify>,
    hold_loads: Arc<AtomicBool>,
    held_loads: Arc<AtomicUsize>,
    release_loads: Arc<Notify>,
}

impl Recording {
    fn ops(&self) -> Vec<String> {
        self.ops.lock().unwrap().clone()
    }

    fn count(&self, op: &str) -> usize {
        self.ops().iter().filter(|o| o.starts_with(op)).count()
    }
}

impl McpSessionStore for Recording {
    fn load(&self, id: &str) -> McpSessionStoreFuture<'_, Option<Vec<u8>>> {
        self.ops.lock().unwrap().push(format!("load {id}"));
        if self.fail_load.load(Ordering::SeqCst) {
            return Box::pin(async { Err("backend down".into()) });
        }
        let id = id.to_owned();
        Box::pin(async move {
            if self.hold_loads.load(Ordering::SeqCst) {
                self.held_loads.fetch_add(1, Ordering::SeqCst);
                self.release_loads.notified().await;
            }
            self.inner.load(&id).await
        })
    }

    fn store(&self, id: &str, record: &[u8], ttl: Duration) -> McpSessionStoreFuture<'_, ()> {
        self.ops.lock().unwrap().push(format!("store {id}"));
        if self.fail_store.load(Ordering::SeqCst) {
            return Box::pin(async { Err("backend down".into()) });
        }
        let record = record.to_vec();
        let id = id.to_owned();
        Box::pin(async move {
            if self.hold_writes.load(Ordering::SeqCst) {
                self.held.fetch_add(1, Ordering::SeqCst);
                self.release.notified().await;
            }
            self.inner.store(&id, &record, ttl).await
        })
    }

    fn delete(&self, id: &str) -> McpSessionStoreFuture<'_, ()> {
        self.ops.lock().unwrap().push(format!("delete {id}"));
        if self.fail_delete.load(Ordering::SeqCst) {
            return Box::pin(async { Err("backend down".into()) });
        }
        self.inner.delete(id)
    }
}

async fn app(yaml: &str, plugin: McpServer, probe: &InitProbe) -> (Routes, McpSessions) {
    let app = AppBuilder::new()
        .override_config(R2eConfig::from_yaml_str(yaml).unwrap())
        .load_config::<()>()
        .plugin(plugin.session_init::<InitProbe>())
        .provide(Calc)
        .provide(CallLog::default())
        .provide(probe.clone())
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>();
    let sessions = app.bean_context().try_get::<McpSessions>().unwrap();
    (app.build(), sessions)
}

/// Two instances on one store.
async fn pair(yaml: &str, store: &Recording) -> (Routes, Routes, McpSessions, InitProbe) {
    let probe = InitProbe::default();
    let (a, _) = app(yaml, McpServer::new().with_session_store(store.clone()), &probe).await;
    let (b, sessions_b) = app(yaml, McpServer::new().with_session_store(store.clone()), &probe).await;
    (a, b, sessions_b, probe)
}

async fn add(router: &Routes, session: &str) -> StatusCode {
    add_with_id(router, session, 7).await
}

/// Concurrent requests on one session need distinct JSON-RPC ids: rmcp
/// keys the in-flight responses by id and drops a duplicate's.
async fn add_with_id(router: &Routes, session: &str, id: u64) -> StatusCode {
    support::post(
        router,
        "/mcp",
        Some(session),
        &json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": "add", "arguments": { "a": 1, "b": 2 } }
        }),
    )
    .await
    .status
}

async fn delete(router: &Routes, session: &str) -> StatusCode {
    let request = Request::builder()
        .method("DELETE")
        .uri("/mcp")
        .header("host", "localhost")
        .header("mcp-session-id", session)
        .header("mcp-protocol-version", support::PROTOCOL_VERSION)
        .body(Body::empty())
        .unwrap();
    router.clone().oneshot(request).await.unwrap().status()
}

/// The eviction after an idle timeout is two steps (worker quits, manager
/// forgets); poll until the status settles on `expected`.
async fn settle(router: &Routes, session: &str, expected: StatusCode) -> StatusCode {
    let mut status = add(router, session).await;
    for _ in 0..100 {
        if status == expected {
            break;
        }
        rt::sleep(Duration::from_millis(10)).await;
        status = add(router, session).await;
    }
    status
}

/// Poll until `counter` reads `expected` (a held operation was entered, a
/// background write landed).
async fn reached(counter: &AtomicUsize, expected: usize) -> bool {
    for _ in 0..100 {
        if counter.load(Ordering::SeqCst) == expected {
            return true;
        }
        rt::sleep(Duration::from_millis(1)).await;
    }
    false
}

const STORE: &str = "mcp:\n  session:\n    store-ttl-secs: 60\n";

// ── Restore ────────────────────────────────────────────────────────────────

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn session_is_restored_on_another_instance() {
    let store = Recording::default();
    let (a, b, sessions_b, probe) = pair(STORE, &store).await;
    let session = support::initialize(&a, "/mcp").await;
    assert!(store.inner.contains(&session), "{:?}", store.ops());
    assert_eq!(probe.runs(), vec![(false, false)]);

    // The other instance has never seen the id: it restores it from the
    // record and serves the call.
    assert_eq!(add(&b, &session).await, StatusCode::OK);
    let live = sessions_b.get(&session).expect("restored session is in McpSessions");
    assert_eq!(live.id().map(|id| id.as_str()), Some(session.as_str()));
    // `session_init` ran again on the restoring instance, flagged as such
    // and with the id known this time.
    assert_eq!(probe.runs(), vec![(false, false), (true, true)]);
    // Once restored, it is a local session: no further load.
    assert_eq!(add(&b, &session).await, StatusCode::OK);
    assert_eq!(store.count("load"), 1, "{:?}", store.ops());
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn store_bean_is_picked_up() {
    let store = Recording::default();
    let probe = InitProbe::default();
    let bean: Arc<dyn McpSessionStore> = Arc::new(store.clone());
    let a = AppBuilder::new()
        .provide(bean)
        .provide(Calc)
        .provide(CallLog::default())
        .provide(probe.clone())
        .plugin(McpServer::new().session_init::<InitProbe>())
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();
    let (b, _, _, _) = pair(STORE, &store).await;

    let session = support::initialize(&a, "/mcp").await;
    assert_eq!(add(&b, &session).await, StatusCode::OK);
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn idle_evicted_session_is_restored_when_a_store_is_set() {
    let store = Recording::default();
    let probe = InitProbe::default();
    let yaml = "mcp:\n  session:\n    idle-timeout-secs: 1\n    store-ttl-secs: 60\n";
    let (a, _) = app(yaml, McpServer::new().with_session_store(store.clone()), &probe).await;
    let session = support::initialize(&a, "/mcp").await;

    // Past the idle timeout: rmcp dropped the session (compare
    // `sessions::idle_session_is_closed_after_the_timeout`); the record
    // brings it back on the next request.
    rt::sleep(Duration::from_millis(2500)).await;
    assert_eq!(settle(&a, &session, StatusCode::OK).await, StatusCode::OK);
    assert!(
        probe.runs().contains(&(true, true)),
        "no restored init: {:?}",
        probe.runs()
    );
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn expired_record_is_not_restored() {
    let store = Recording::default();
    let (a, b, _, _) = pair("mcp:\n  session:\n    store-ttl-secs: 2\n", &store).await;
    let session = support::initialize(&a, "/mcp").await;

    rt::sleep(Duration::from_secs(3)).await;
    assert_eq!(add(&b, &session).await, StatusCode::NOT_FOUND);
    assert!(!store.inner.contains(&session));
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn active_session_is_re_armed_at_half_life() {
    let store = Recording::default();
    let yaml = "mcp:\n  session:\n    idle-timeout-secs: 0\n    store-ttl-secs: 4\n";
    let (a, b, _, _) = pair(yaml, &store).await;
    let session = support::initialize(&a, "/mcp").await;

    // Traffic every 1.5 s: the record, written at t=0, is re-armed by the
    // requests at t=3, 6 and 9 (the first past each half-life) — never
    // by the ones in between.
    for _ in 0..7 {
        rt::sleep(Duration::from_millis(1500)).await;
        assert_eq!(add(&a, &session).await, StatusCode::OK);
    }
    assert_eq!(store.count("store"), 4, "{:?}", store.ops());
    // t=10.5: a 4 s record written at t=0 would be long gone.
    assert_eq!(add(&b, &session).await, StatusCode::OK);
}

// ── Delete ─────────────────────────────────────────────────────────────────

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn record_written_at_initialize_and_removed_on_delete() {
    let store = Recording::default();
    let (a, b, _, _) = pair(STORE, &store).await;
    let session = support::initialize(&a, "/mcp").await;
    assert_eq!(store.ops(), vec![format!("store {session}")]);

    assert!(delete(&a, &session).await.is_success());
    assert!(!store.inner.contains(&session));
    assert_eq!(store.count("delete"), 1, "{:?}", store.ops());
    assert_eq!(add(&b, &session).await, StatusCode::NOT_FOUND);
    assert_eq!(add(&a, &session).await, StatusCode::NOT_FOUND);
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn in_flight_re_arm_does_not_resurrect_a_deleted_record() {
    let store = Recording::default();
    let yaml = "mcp:\n  session:\n    idle-timeout-secs: 0\n    store-ttl-secs: 4\n";
    let (a, b, _, _) = pair(yaml, &store).await;
    let session = support::initialize(&a, "/mcp").await;

    // A request past the half-life schedules a re-arm; hold its write.
    store.hold_writes.store(true, Ordering::SeqCst);
    rt::sleep(Duration::from_secs(3)).await;
    assert_eq!(add(&a, &session).await, StatusCode::OK);
    for _ in 0..100 {
        if store.held.load(Ordering::SeqCst) == 1 {
            break;
        }
        rt::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(store.held.load(Ordering::SeqCst), 1, "{:?}", store.ops());

    // The session ends while the write is pending …
    assert!(delete(&a, &session).await.is_success());
    assert!(!store.inner.contains(&session));
    // … then the write lands: the record must not come back.
    store.release.notify_one();
    for _ in 0..100 {
        if store.count("delete") == 2 {
            break;
        }
        rt::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(store.count("delete"), 2, "{:?}", store.ops());
    assert!(!store.inner.contains(&session));
    assert_eq!(add(&b, &session).await, StatusCode::NOT_FOUND);
}

/// The backend refuses the `delete`: rmcp still answers 202, and this
/// instance remembers the deletion (tombstone) — but the record survives,
/// so another instance restores it until its TTL runs out. No
/// cross-instance eviction without the backend's help: a documented limit.
#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn delete_backend_failure_leaves_a_local_tombstone_only() {
    let store = Recording::default();
    let (a, b, _, _) = pair(STORE, &store).await;
    let session = support::initialize(&a, "/mcp").await;

    store.fail_delete.store(true, Ordering::SeqCst);
    assert_eq!(delete(&a, &session).await, StatusCode::ACCEPTED);
    assert!(store.inner.contains(&session), "{:?}", store.ops());
    // Gone here: the local copy is closed and the tombstone blocks a
    // restore without even asking the backend.
    let loads = store.count("load");
    assert_eq!(add(&a, &session).await, StatusCode::NOT_FOUND);
    assert_eq!(store.count("load"), loads, "{:?}", store.ops());
    // Still there for an instance that never saw the DELETE.
    assert_eq!(add(&b, &session).await, StatusCode::OK);
}

// ── Restore: concurrency ───────────────────────────────────────────────────

/// Two requests name a session this instance does not hold: rmcp restores
/// it once (the second waits for the first) and serves both — one `load`.
#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn concurrent_requests_share_one_restore() {
    let store = Recording::default();
    let (a, b, sessions_b, _) = pair(STORE, &store).await;
    let session = support::initialize(&a, "/mcp").await;

    store.hold_loads.store(true, Ordering::SeqCst);
    let first = {
        let (b, session) = (b.clone(), session.clone());
        rt::spawn(async move { add(&b, &session).await })
    };
    assert!(reached(&store.held_loads, 1).await, "{:?}", store.ops());
    let second = {
        let (b, session) = (b.clone(), session.clone());
        rt::spawn(async move { add_with_id(&b, &session, 8).await })
    };
    // Let the second request reach rmcp's wait on the pending restore.
    rt::sleep(Duration::from_millis(20)).await;
    store.hold_loads.store(false, Ordering::SeqCst);
    store.release_loads.notify_one();

    assert_eq!(first.await.unwrap(), StatusCode::OK);
    assert_eq!(second.await.unwrap(), StatusCode::OK);
    assert_eq!(store.count("load"), 1, "{:?}", store.ops());
    assert!(sessions_b.get(&session).is_some(), "{:?}", store.ops());
}

// ── Handshake ──────────────────────────────────────────────────────────────

/// `initialize` answered an error (the init hook failed): the session rmcp
/// opened is unusable, so nothing is written — a later, successful
/// handshake is.
#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn failed_handshake_is_not_persisted() {
    let store = Recording::default();
    let probe = InitProbe::default();
    let (a, _) = app(STORE, McpServer::new().with_session_store(store.clone()), &probe).await;

    probe.fail_next.store(true, Ordering::SeqCst);
    let failed = support::post(&a, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(failed.status, StatusCode::OK, "{}", failed.raw_body);
    assert!(
        failed.message()["error"].is_object(),
        "expected a JSON-RPC error: {}",
        failed.raw_body
    );
    assert_eq!(store.count("store"), 0, "{:?}", store.ops());

    let session = support::initialize(&a, "/mcp").await;
    assert_eq!(store.ops(), vec![format!("store {session}")]);
}

// ── The store never pre-empts rmcp ─────────────────────────────────────────

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn not_acceptable_takes_precedence_over_the_restore() {
    let store = Recording::default();
    let (a, b, _, _) = pair(STORE, &store).await;
    let session = support::initialize(&a, "/mcp").await;

    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "text/plain")
        .header("mcp-session-id", &session)
        .body(Body::from(
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string(),
        ))
        .unwrap();
    let status = b.clone().oneshot(request).await.unwrap().status();
    assert_eq!(status, StatusCode::NOT_ACCEPTABLE);
    assert_eq!(store.count("load"), 0, "{:?}", store.ops());
}

// ── Failures ───────────────────────────────────────────────────────────────

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn store_load_error_is_a_500() {
    let store = Recording::default();
    let (a, b, _, _) = pair(STORE, &store).await;
    let session = support::initialize(&a, "/mcp").await;

    store.fail_load.store(true, Ordering::SeqCst);
    assert_eq!(add(&b, &session).await, StatusCode::INTERNAL_SERVER_ERROR);
    // The owning instance is unaffected.
    assert_eq!(add(&a, &session).await, StatusCode::OK);
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn store_write_error_keeps_the_session_local() {
    let store = Recording::default();
    let (a, b, _, _) = pair(STORE, &store).await;
    store.fail_store.store(true, Ordering::SeqCst);

    let session = support::initialize(&a, "/mcp").await;
    assert_eq!(add(&a, &session).await, StatusCode::OK);
    assert_eq!(add(&b, &session).await, StatusCode::NOT_FOUND);
}

// ── Boot ───────────────────────────────────────────────────────────────────

#[r2e_core::test]
#[should_panic(expected = "a session store needs sessions")]
async fn stateless_with_a_builder_store_is_a_boot_error() {
    let _ = AppBuilder::new()
        .provide(Calc)
        .provide(CallLog::default())
        .plugin(
            McpServer::new()
                .stateless(true)
                .with_session_store(InMemoryMcpSessionStore::new()),
        )
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();
}

#[r2e_core::test]
#[should_panic(expected = "a session store needs sessions")]
async fn stateless_with_a_store_bean_is_a_boot_error() {
    let bean: Arc<dyn McpSessionStore> = Arc::new(InMemoryMcpSessionStore::new());
    let _ = AppBuilder::new()
        .provide(bean)
        .provide(Calc)
        .provide(CallLog::default())
        .plugin(McpServer::new().stateless(true))
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();
}

#[r2e_core::test]
#[should_panic(expected = "store-ttl-secs (McpServer::with_session_store_ttl) must be > 0")]
async fn zero_store_ttl_is_a_boot_error() {
    let _ = AppBuilder::new()
        .override_config(
            R2eConfig::from_yaml_str("mcp:\n  session:\n    store-ttl-secs: 0\n").unwrap(),
        )
        .load_config::<()>()
        .provide(Calc)
        .provide(CallLog::default())
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn builder_ttl_overrides_config() {
    let store = Recording::default();
    let probe = InitProbe::default();
    let yaml = "mcp:\n  session:\n    store-ttl-secs: 2\n";
    let (a, _) = app(
        yaml,
        McpServer::new()
            .with_session_store(store.clone())
            .with_session_store_ttl(Duration::from_secs(60)),
        &probe,
    )
    .await;
    let (b, _, _, _) = pair(STORE, &store).await;
    let session = support::initialize(&a, "/mcp").await;

    rt::sleep(Duration::from_secs(3)).await;
    assert_eq!(add(&b, &session).await, StatusCode::OK);
}

// ── Loopback ───────────────────────────────────────────────────────────────

#[cfg(feature = "loopback")]
#[r2e_core::test]
async fn loopback_session_is_never_written() {
    use r2e_mcp::{LoopbackIdentity, McpLoopback};

    let store = Recording::default();
    let app = AppBuilder::new()
        .provide(Calc)
        .provide(CallLog::default())
        .plugin(McpServer::new().with_session_store(store.clone()))
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>();
    let loopback = app.bean_context().try_get::<McpLoopback>().unwrap();
    let _router = app.build();

    let session = loopback
        .connect("agent-1", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
    session.list_tools(None).await.unwrap();
    assert!(store.ops().is_empty(), "{:?}", store.ops());
}
