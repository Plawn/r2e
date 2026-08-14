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
    /// Fail the first `n` calls, then succeed.
    FailTimes(usize),
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
        Self {
            inner: Arc::new(ScriptedInner {
                default,
                script: Mutex::new(HashMap::new()),
                creates: AtomicUsize::new(0),
                disposals: Mutex::new(Vec::new()),
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

    fn dispose<'a>(&'a self, tenant: &'a TenantId, _value: Resource) -> BoxFuture<'a, ()> {
        Box::pin(async move {
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
