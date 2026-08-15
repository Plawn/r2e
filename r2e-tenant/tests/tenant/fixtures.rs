//! Fixtures shared by the tenancy test modules.
//!
//! The recurring shape is a **scripted source**: a `TenantSource` whose answer
//! per tenant is configured up front (a value, "unknown", an error, or a delay)
//! and which records every call, so a test can assert both what the caller saw
//! and how many times the source ran.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e_core::http::{Body, Request, Response, Router, StatusCode};
use r2e_core::BeanContext;
use r2e_tenant::{BoxError, BoxFuture, TenantContext, TenantId, TenantSource, Tenanted};
use tower::ServiceExt;

/// A per-tenant resource: a cheap handle carrying the tenant it was built for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub tenant: String,
    pub generation: usize,
}

impl Resource {
    pub fn new(tenant: &TenantId, generation: usize) -> Self {
        Self {
            tenant: tenant.as_str().to_string(),
            generation,
        }
    }
}

/// What a scripted source does for a given tenant.
#[derive(Clone)]
pub enum Behaviour {
    /// Build a resource.
    Ok,
    /// Report the tenant as not provisioned (`Ok(None)`).
    Unknown,
    /// Fail with this message.
    Fail(String),
    /// Sleep this long, then build a resource.
    Slow(Duration),
    /// Sleep this long, then report the tenant as not provisioned.
    SlowUnknown(Duration),
    /// Fail the first `n` calls, then succeed.
    FailTimes(usize),
    /// Park at a [`Gate`] until the test releases it, then build a resource.
    Gated(Arc<Gate>),
    /// Park at a [`Gate`] until the test releases it, then fail.
    ///
    /// Lets a test queue waiters *behind* a creation that is going to fail,
    /// which is what the "waiters take the initializer over in turn" path needs.
    GatedFail(Arc<Gate>),
    /// Park at a [`Gate`] until the test releases it, then report the tenant as
    /// not provisioned (`Ok(None)`).
    ///
    /// Lets a test hold an "unknown" verdict *inside* the source while other
    /// resolutions for the same tenant run to completion — the interleaving the
    /// negative cache has to survive.
    GatedUnknown(Arc<Gate>),
    /// Panic inside `create`.
    Panic,
}

/// A creation the test drives by hand: it can wait for `create` to be *inside*
/// the source, act while it is parked there, and then let it finish.
///
/// Sleeps would make the eviction/drain race tests timing-dependent; this makes
/// the interleaving exact.
pub struct Gate {
    /// One permit per `create` that reached the gate.
    started: tokio::sync::Semaphore,
    /// Permits handed out by `release`.
    open: tokio::sync::Semaphore,
}

impl Gate {
    /// A closed gate.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            started: tokio::sync::Semaphore::new(0),
            open: tokio::sync::Semaphore::new(0),
        })
    }

    /// Wait until a `create` call is parked at the gate.
    pub async fn wait_started(&self) {
        self.started.acquire().await.unwrap().forget();
    }

    /// Let every parked (and future) `create` through.
    pub fn release(&self) {
        self.open.add_permits(1024);
    }

    pub(crate) async fn enter(&self) {
        self.started.add_permits(1);
        self.open.acquire().await.unwrap().forget();
    }
}

/// A `TenantSource<Resource>` with a per-tenant script and call counters.
#[derive(Clone)]
pub struct ScriptedSource {
    inner: Arc<ScriptedInner>,
}

struct ScriptedInner {
    default: Behaviour,
    script: Mutex<HashMap<String, Behaviour>>,
    creates: AtomicUsize,
    disposals: Mutex<Vec<String>>,
    /// Every disposed value, identified by `(tenant, generation)` — the same
    /// resource disposed twice appears twice.
    disposed_values: Mutex<Vec<(String, usize)>>,
    dispose_delay: Duration,
    /// When set, `dispose` parks here before it records anything.
    dispose_gate: Option<Arc<Gate>>,
    generation: AtomicUsize,
    attempts: Mutex<HashMap<String, usize>>,
}

impl ScriptedSource {
    /// A source that builds a resource for every tenant.
    pub fn new() -> Self {
        Self::with_default(Behaviour::Ok)
    }

    /// A source whose unscripted tenants get `default`.
    pub fn with_default(default: Behaviour) -> Self {
        Self::with_dispose_delay(default, Duration::ZERO)
    }

    /// A source whose `dispose` takes this long — wide enough for two removals
    /// to be inside it at once.
    /// A source whose `dispose` parks at `gate` until it is released — the
    /// disposal counterpart of [`Behaviour::Gated`], for tests that need to act
    /// while a close is in flight.
    pub fn with_dispose_gate(default: Behaviour, gate: Arc<Gate>) -> Self {
        let mut source = Self::with_dispose_delay(default, Duration::ZERO);
        Arc::get_mut(&mut source.inner)
            .expect("the source is not shared yet")
            .dispose_gate = Some(gate);
        source
    }

    pub fn with_dispose_delay(default: Behaviour, dispose_delay: Duration) -> Self {
        Self {
            inner: Arc::new(ScriptedInner {
                default,
                script: Mutex::new(HashMap::new()),
                creates: AtomicUsize::new(0),
                disposals: Mutex::new(Vec::new()),
                disposed_values: Mutex::new(Vec::new()),
                dispose_delay,
                dispose_gate: None,
                generation: AtomicUsize::new(0),
                attempts: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Script one tenant.
    pub fn on(self, tenant: &str, behaviour: Behaviour) -> Self {
        self.inner
            .script
            .lock()
            .unwrap()
            .insert(tenant.to_string(), behaviour);
        self
    }

    /// Rescript a tenant after the source is wired.
    pub fn set(&self, tenant: &str, behaviour: Behaviour) {
        self.inner
            .script
            .lock()
            .unwrap()
            .insert(tenant.to_string(), behaviour);
    }

    /// How many times `create` ran.
    pub fn creates(&self) -> usize {
        self.inner.creates.load(Ordering::SeqCst)
    }

    /// The tenants `dispose` was called for, in order.
    pub fn disposals(&self) -> Vec<String> {
        self.inner.disposals.lock().unwrap().clone()
    }

    /// The tenants `dispose` was called for, sorted.
    pub fn sorted_disposals(&self) -> Vec<String> {
        let mut disposals = self.disposals();
        disposals.sort();
        disposals
    }

    /// How many *values* were handed to `dispose` more than once.
    ///
    /// `TenantSource::dispose` is documented as being called at most once per
    /// cached value, so every race test asserts this is zero.
    pub fn double_disposals(&self) -> usize {
        let disposed = self.inner.disposed_values.lock().unwrap();
        let mut seen: HashMap<(String, usize), usize> = HashMap::new();
        for value in disposed.iter() {
            *seen.entry(value.clone()).or_insert(0) += 1;
        }
        seen.values().filter(|count| **count > 1).count()
    }
}

impl Default for ScriptedSource {
    fn default() -> Self {
        Self::new()
    }
}

impl TenantSource<Resource> for ScriptedSource {
    fn create<'a>(
        &'a self,
        tenant: &'a TenantId,
        _ctx: &'a TenantContext<'a>,
    ) -> BoxFuture<'a, Result<Option<Resource>, BoxError>> {
        Box::pin(async move {
            self.inner.creates.fetch_add(1, Ordering::SeqCst);
            let behaviour = self
                .inner
                .script
                .lock()
                .unwrap()
                .get(tenant.as_str())
                .cloned()
                .unwrap_or_else(|| self.inner.default.clone());
            match behaviour {
                Behaviour::Ok => Ok(Some(self.build(tenant))),
                Behaviour::Unknown => Ok(None),
                Behaviour::Fail(message) => Err(message.into()),
                Behaviour::Slow(delay) => {
                    tokio::time::sleep(delay).await;
                    Ok(Some(self.build(tenant)))
                }
                Behaviour::SlowUnknown(delay) => {
                    tokio::time::sleep(delay).await;
                    Ok(None)
                }
                Behaviour::Gated(gate) => {
                    gate.enter().await;
                    Ok(Some(self.build(tenant)))
                }
                Behaviour::GatedFail(gate) => {
                    gate.enter().await;
                    Err(format!("gated failure for `{tenant}`").into())
                }
                Behaviour::GatedUnknown(gate) => {
                    gate.enter().await;
                    Ok(None)
                }
                Behaviour::Panic => panic!("scripted panic while creating `{tenant}`"),
                Behaviour::FailTimes(limit) => {
                    let attempt = {
                        let mut attempts = self.inner.attempts.lock().unwrap();
                        let counter = attempts.entry(tenant.as_str().to_string()).or_insert(0);
                        *counter += 1;
                        *counter
                    };
                    if attempt <= limit {
                        Err(format!("attempt {attempt} failed").into())
                    } else {
                        Ok(Some(self.build(tenant)))
                    }
                }
            }
        })
    }

    fn dispose<'a>(&'a self, tenant: &'a TenantId, value: Resource) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Some(gate) = &self.inner.dispose_gate {
                gate.enter().await;
            }
            self.inner
                .disposed_values
                .lock()
                .unwrap()
                .push((value.tenant.clone(), value.generation));
            if !self.inner.dispose_delay.is_zero() {
                tokio::time::sleep(self.inner.dispose_delay).await;
            }
            self.inner
                .disposals
                .lock()
                .unwrap()
                .push(tenant.as_str().to_string());
        })
    }
}

impl ScriptedSource {
    fn build(&self, tenant: &TenantId) -> Resource {
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst);
        Resource::new(tenant, generation)
    }
}

/// A wired map over a fresh scripted source, with default settings.
pub fn map_with(
    source: ScriptedSource,
    settings: r2e_tenant::TenantedSettings,
) -> (Tenanted<Resource>, ScriptedSource) {
    let map = Tenanted::new(
        Arc::new(source.clone()),
        Arc::new(BeanContext::empty()),
        settings,
        None,
    );
    (map, source)
}

/// A tenant id, unwrapped — every literal in the tests is a valid id.
pub fn tid(raw: &str) -> TenantId {
    TenantId::parse(raw).expect("valid tenant id")
}

/// Poll until `condition` holds, failing the test after ~2 seconds.
///
/// For the background work the map does on its own — the detached `max-active`
/// trim, `invalidate`'s spawned disposal — which has no handle to await.
pub async fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..400 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

// ── Router driving ─────────────────────────────────────────────────────────

/// Drive one request through a router and return `(status, body)`.
pub async fn send(
    router: Router,
    path: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, String) {
    let response = raw(router, path, headers).await;
    let status = response.status();
    (status, body_string(response).await)
}

/// Drive one request through a router and return the raw response.
pub async fn raw(router: Router, path: &str, headers: &[(&str, &str)]) -> Response {
    let mut request = Request::builder().uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    router
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

/// Collect a response body into a `String`.
pub async fn body_string(response: Response) -> String {
    use http_body_util::BodyExt;
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}
