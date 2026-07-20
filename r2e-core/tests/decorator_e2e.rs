//! End-to-end coverage for graph-resolved decorators (Phase 6).
//!
//! A hand-written `Controller` impl mirroring exactly what `#[routes]`
//! emits: decorators built **once** inside `routes(state, core, ctx)` via
//! `<Spec as DecoratorSpec>::build(expr, ctx)`, wrapped in one `Arc` per
//! route and captured by the handler closure. Asserts the properties the
//! macro-level tests cannot see directly:
//!
//! - build-once (the spec's `build` runs exactly once across N requests);
//! - the `Deps` fold shape (`ContextConstruct::Deps` ++ every site's
//!   `Spec::Deps`) accepted by the real `AllSatisfied` bound at
//!   `register_controller()`;
//! - spec build is independent of the state provision list `P`
//!   (module-private guard deps behave like private core deps);
//! - guard declaration order, short-circuit before interceptors, and the
//!   per-request cost model (one `Arc` clone + monomorphized calls).
//!
//! (Origin: the Phase 6a spike; kept as a permanent regression test, now on
//! the real `Guard`/`Interceptor` traits.)

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use r2e_core::beans::{BeanContext, BeanRegistry};
use r2e_core::guards::{GuardContext, GuardError, PathParams};
use r2e_core::http::response::{IntoResponse, Response};
use r2e_core::http::{HeaderMap, Router, StatusCode, Uri};
use r2e_core::prelude::*;
use r2e_core::type_list::{TAppend, TCons, TNil};
use r2e_core::{AppBuilder, Controller, DecoratorSpec, Identity, NoIdentity, SelfBuilt};

// ── Beans ──────────────────────────────────────────────────────────────────

/// Bean dep of the rate-limit guard. Counts spec builds and guard checks so
/// the tests can assert build-once / check-per-request.
#[derive(Clone)]
struct SpikeRegistry {
    builds: Arc<AtomicUsize>,
    hits: Arc<AtomicUsize>,
}

impl SpikeRegistry {
    fn new() -> Self {
        Self {
            builds: Arc::new(AtomicUsize::new(0)),
            hits: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// Bean dep of the audit interceptor.
#[derive(Clone)]
struct AuditLog(Arc<Mutex<Vec<String>>>);

/// Ordinary controller dep, to prove core deps and decorator deps fold into
/// one list.
#[derive(Clone)]
struct SpikeService(&'static str);

// ── Config-type specs (bean-reading decorators) ────────────────────────────

/// Pure config value; the graph dep is injected in `build`.
struct RateLimitCfg {
    max: usize,
}

impl RateLimitCfg {
    fn per_user(max: usize) -> Self {
        Self { max }
    }
}

struct SpikeRateLimitGuard {
    registry: SpikeRegistry,
    max: usize,
}

impl DecoratorSpec for RateLimitCfg {
    type Product = SpikeRateLimitGuard;
    type Deps = TCons<SpikeRegistry, TNil>;

    fn build(self, ctx: &BeanContext) -> SpikeRateLimitGuard {
        let registry: SpikeRegistry = ctx.get();
        registry.builds.fetch_add(1, Ordering::SeqCst);
        SpikeRateLimitGuard {
            registry,
            max: self.max,
        }
    }
}

impl<I: Identity> Guard<I> for SpikeRateLimitGuard {
    fn check(
        &self,
        _ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Response>> + Send {
        async move {
            if self.registry.hits.fetch_add(1, Ordering::SeqCst) >= self.max {
                Err(GuardError::new(StatusCode::TOO_MANY_REQUESTS, "rate limited").into())
            } else {
                Ok(())
            }
        }
    }
}

struct AuditCfg {
    channel: &'static str,
}

impl AuditCfg {
    fn channel(channel: &'static str) -> Self {
        Self { channel }
    }
}

struct AuditInterceptor {
    log: AuditLog,
    channel: &'static str,
}

impl DecoratorSpec for AuditCfg {
    type Product = AuditInterceptor;
    type Deps = TCons<AuditLog, TNil>;

    fn build(self, ctx: &BeanContext) -> AuditInterceptor {
        AuditInterceptor {
            log: ctx.get(),
            channel: self.channel,
        }
    }
}

impl<R: Send> Interceptor<R> for AuditInterceptor {
    fn around<F, Fut>(
        &self,
        ctx: InterceptorContext,
        next: F,
    ) -> impl std::future::Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = R> + Send,
    {
        async move {
            self.log
                .0
                .lock()
                .unwrap()
                .push(format!("{}:enter {}", self.channel, ctx.method_name));
            let out = next().await;
            self.log
                .0
                .lock()
                .unwrap()
                .push(format!("{}:exit {}", self.channel, ctx.method_name));
            out
        }
    }
}

// ── Self-contained decorator via the real SelfBuilt marker ─────────────────

struct RequireHeader(&'static str);

impl SelfBuilt for RequireHeader {}

impl<I: Identity> Guard<I> for RequireHeader {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Response>> + Send {
        async move {
            if ctx.headers.contains_key(self.0) {
                Ok(())
            } else {
                Err(GuardError::forbidden(format!("missing header {}", self.0)).into())
            }
        }
    }
}

// ── Hand-written controller mirroring 6c output ────────────────────────────

/// Sites on the handler, in declaration order (what the attributes will be):
/// `#[guard(RequireHeader("x-spike"))]`, `#[guard(RateLimitCfg::per_user(2))]`,
/// `#[intercept(AuditCfg::channel("audit"))]`.
struct SpikeDecorators {
    g_header: RequireHeader,
    g_limit: SpikeRateLimitGuard,
    i_audit: AuditInterceptor,
}

impl SpikeDecorators {
    /// One `<Spec as DecoratorSpec>::build(#expr, ctx)` per site — exactly
    /// the macro emission shape.
    fn build(ctx: &BeanContext) -> Self {
        Self {
            g_header: <RequireHeader as DecoratorSpec>::build(RequireHeader("x-spike"), ctx),
            g_limit: <RateLimitCfg as DecoratorSpec>::build(RateLimitCfg::per_user(2), ctx),
            i_audit: <AuditCfg as DecoratorSpec>::build(AuditCfg::channel("audit"), ctx),
        }
    }
}

struct SpikeController {
    service: SpikeService,
}

impl SpikeController {
    async fn list(&self) -> String {
        format!("svc:{}", self.service.0)
    }
}

impl ContextConstruct for SpikeController {
    type Deps = TCons<SpikeService, TNil>;

    fn from_context(ctx: &BeanContext) -> Self {
        Self { service: ctx.get() }
    }
}

/// The fold `#[routes]` will emit as `Controller::Deps`: core deps ++ every
/// site's spec deps, via `TAppend` projections (all lists concrete — no extra
/// bounds needed for normalization).
type SpikeDeps = <<SpikeController as ContextConstruct>::Deps as TAppend<
    <<RequireHeader as DecoratorSpec>::Deps as TAppend<
        <<RateLimitCfg as DecoratorSpec>::Deps as TAppend<
            <AuditCfg as DecoratorSpec>::Deps,
        >>::Output,
    >>::Output,
>>::Output;

impl<S> Controller<S> for SpikeController
where
    S: Clone + Send + Sync + 'static,
{
    type Deps = SpikeDeps;

    fn construct(_state: &S, ctx: &BeanContext) -> Self {
        <Self as ContextConstruct>::from_context(ctx)
    }

    fn routes(_state: &S, core: Arc<Self>, ctx: &BeanContext) -> Router<S> {
        // Decorators built HERE, once, from the resolved graph — then one
        // Arc of the route's site set moved into the closure. Axum requires
        // handler closures to be Clone — Arc satisfies it; the per-request
        // cost is the Arc clone (replaces today's per-request spec
        // evaluation + BeanLookup TypeId chain).
        let deco = Arc::new(SpikeDecorators::build(ctx));
        Router::new().route(
            "/spike",
            r2e_core::http::routing::get(move |headers: HeaderMap, uri: Uri| {
                let deco = deco.clone();
                let core = core.clone();
                async move {
                    let guard_ctx = GuardContext::<NoIdentity> {
                        method_name: "list",
                        controller_name: "SpikeController",
                        headers: &headers,
                        uri: &uri,
                        path_params: PathParams::EMPTY,
                        identity: None,
                    };
                    // Guards in declaration order, monomorphized field access.
                    if let Err(resp) = deco.g_header.check(&guard_ctx).await {
                        return resp;
                    }
                    if let Err(resp) = deco.g_limit.check(&guard_ctx).await {
                        return resp;
                    }
                    // Interceptor chain sees the raw return type.
                    let out = deco
                        .i_audit
                        .around(
                            InterceptorContext {
                                method_name: "list",
                                controller_name: "SpikeController",
                            },
                            || async { core.list().await },
                        )
                        .await;
                    IntoResponse::into_response(out)
                }
            }),
        )
    }
}

// ── Type-equality helper ────────────────────────────────────────────────────

trait SameTy<B> {}
impl<A> SameTy<A> for A {}
fn assert_same<A: SameTy<B>, B>() {}

// ── Tests ───────────────────────────────────────────────────────────────────

/// s1 — the Deps fold: core deps ++ spec deps, self-contained specs
/// contributing nothing.
#[test]
fn deps_fold_type_level() {
    assert_same::<SpikeDeps, TCons<SpikeService, TCons<SpikeRegistry, TCons<AuditLog, TNil>>>>();
}

/// s2 — decorators build from the **context**, independently of the state
/// provision list `P`: a module-private bean dep works exactly like a
/// module-private core dep (no bean-backed-extractor asymmetry).
#[r2e_core::test]
async fn decorators_build_from_context_not_state() {
    let mut registry = BeanRegistry::new();
    registry.provide(SpikeRegistry::new());
    let ctx = registry.resolve().await.expect("graph must resolve");

    let guard = <RateLimitCfg as DecoratorSpec>::build(RateLimitCfg::per_user(1), &ctx);
    let headers = HeaderMap::new();
    let uri: Uri = "/x".parse().unwrap();
    let guard_ctx = GuardContext::<NoIdentity> {
        method_name: "m",
        controller_name: "c",
        headers: &headers,
        uri: &uri,
        path_params: PathParams::EMPTY,
        identity: None,
    };
    assert!(guard.check(&guard_ctx).await.is_ok());
    assert!(
        guard.check(&guard_ctx).await.is_err(),
        "budget of 1 exhausted"
    );
}

/// s3 — end to end through the real `register_controller()` (witnesses
/// inferred; the folded Deps checked by the real `AllSatisfied` bound):
/// build-once, guard order, short-circuit before interceptors, rate limit,
/// and per-request cost = Arc clone + field access.
#[r2e_core::test]
async fn decorators_end_to_end() {
    let registry = SpikeRegistry::new();
    let audit = AuditLog(Arc::new(Mutex::new(Vec::new())));

    let app = AppBuilder::new()
        .provide(SpikeService("spike"))
        .provide(registry.clone())
        .provide(audit.clone())
        .build_state()
        .await;

    let router = app.register_controller::<SpikeController>().build();

    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let send = |with_header: bool| {
        let router = router.clone();
        async move {
            let mut req = r2e_core::http::Request::builder().uri("/spike");
            if with_header {
                req = req.header("x-spike", "1");
            }
            let resp = router
                .oneshot(req.body(r2e_core::http::Body::empty()).unwrap())
                .await
                .unwrap();
            let status = resp.status();
            let body = resp.into_body().collect().await.unwrap().to_bytes();
            (status, String::from_utf8(body.to_vec()).unwrap())
        }
    };

    // Header guard first (declaration order): short-circuits without
    // touching the rate budget or the interceptor.
    let (status, _) = send(false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(registry.hits.load(Ordering::SeqCst), 0);
    assert!(audit.0.lock().unwrap().is_empty());

    // Two requests within the budget.
    for _ in 0..2 {
        let (status, body) = send(true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "svc:spike");
    }

    // Budget exhausted → 429, and the interceptor never ran for it.
    let (status, _) = send(true).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    // Specs were built exactly once despite 4 requests.
    assert_eq!(registry.builds.load(Ordering::SeqCst), 1);
    // Interceptor ran for the two 200s only, enter+exit each.
    assert_eq!(
        *audit.0.lock().unwrap(),
        vec![
            "audit:enter list",
            "audit:exit list",
            "audit:enter list",
            "audit:exit list"
        ]
    );
}
