//! Proxy-shaped routing: `#[any]` (all-methods routes, wildcard paths, raw
//! `Request` access, streaming responses) and `#[fallback]` (controller-scoped
//! catch-all registered as the app-wide `Router::fallback`).
//!
//! These are the first-class replacements for the raw-axum escape hatch that
//! proxy apps (registry proxies, gateways) previously needed: the handlers
//! below take the full request (method + URI + headers + streaming body),
//! return streamed bodies / redirects, and still get DI, guards, and
//! interceptors from the controller machinery.

use futures_util::StreamExt;
use http_body_util::BodyExt;
use r2e_core::http::extract::Request as AxumRequest;
use r2e_core::http::header::Parts;
use r2e_core::http::response::{IntoResponse, Redirect, Response};
use r2e_core::http::{Body, Bytes, Request, StatusCode};
use r2e_core::prelude::*;
use r2e_core::{Guard, GuardContext, Identity, Interceptor, InterceptorContext};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

async fn body_string(resp: Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

async fn send(
    router: r2e_core::http::Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Body,
) -> (StatusCode, String) {
    let mut b = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        b = b.header(*name, *value);
    }
    let resp = router.oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    (status, body_string(resp).await)
}

// ── 1. #[any]: all methods, wildcard path, raw Request, streamed echo ──────

#[controller]
struct AnyProxyController {}

#[routes]
impl AnyProxyController {
    /// patina-shaped endpoint: routes on the raw request, streams the body back.
    #[any("/proxy/{*path}")]
    async fn proxy(&self, req: AxumRequest) -> Response {
        let method = req.method().clone();
        let path = req.uri().path().to_string();
        let body = req.into_body();
        // Prefix stream + request body stream, passed through untouched.
        let prefix = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(
            format!("{method} {path} "),
        ))]);
        let echoed = body
            .into_data_stream()
            .map(|chunk| chunk.map_err(std::io::Error::other));
        Response::builder()
            .status(200)
            .body(Body::from_stream(prefix.chain(echoed)))
            .unwrap()
    }
}

#[r2e_core::test]
async fn any_route_matches_every_method_and_streams() {
    let router = r2e_core::AppBuilder::new()
        .build_state()
        .await
        .register_controller::<AnyProxyController>()
        .build();

    for method in ["GET", "POST", "PUT", "DELETE", "PATCH"] {
        let (status, body) = send(
            router.clone(),
            method,
            "/proxy/a/b/c",
            &[],
            Body::from("payload"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("{method} /proxy/a/b/c payload"));
    }

    // Wildcard needs at least one segment; outside the prefix nothing matches.
    let (status, _) = send(router, "GET", "/other", &[], Body::empty()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── 2. Raw Request param on a plain verb route ─────────────────────────────

#[controller]
struct RawGetController {}

#[routes]
impl RawGetController {
    #[get("/raw")]
    async fn raw(&self, req: AxumRequest) -> String {
        format!(
            "{}?{}",
            req.uri().path(),
            req.uri().query().unwrap_or_default()
        )
    }
}

#[r2e_core::test]
async fn raw_request_param_works_on_get() {
    let router = r2e_core::AppBuilder::new()
        .build_state()
        .await
        .register_controller::<RawGetController>()
        .build();

    let (status, body) = send(router, "GET", "/raw?x=1", &[], Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "/raw?x=1");
}

// ── 3. #[fallback]: catch-all with DI, streaming, and redirects ────────────

#[derive(Clone)]
struct Greeting(String);

#[controller]
struct FallbackController {
    #[inject]
    greeting: Greeting,
}

#[routes]
impl FallbackController {
    #[get("/known")]
    async fn known(&self) -> &'static str {
        "known"
    }

    #[fallback]
    async fn dispatch(&self, req: AxumRequest) -> Response {
        match req.uri().path() {
            "/redirect-me" => Redirect::temporary("/known").into_response(),
            "/stream-me" => {
                let stream = futures_util::stream::iter(vec![
                    Ok::<_, std::io::Error>(Bytes::from("part1|")),
                    Ok(Bytes::from("part2")),
                ]);
                Response::builder()
                    .status(200)
                    .body(Body::from_stream(stream))
                    .unwrap()
            }
            path => (
                StatusCode::NOT_FOUND,
                format!("{}: no route for {} {}", self.greeting.0, req.method(), path),
            )
                .into_response(),
        }
    }
}

async fn fallback_router() -> r2e_core::http::Router {
    r2e_core::AppBuilder::new()
        .provide(Greeting("sorry".to_string()))
        .build_state()
        .await
        .register_controller::<FallbackController>()
        .build()
}

#[r2e_core::test]
async fn fallback_handles_unmatched_with_bean_access() {
    let router = fallback_router().await;

    // Declared routes win.
    let (status, body) = send(router.clone(), "GET", "/known", &[], Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "known");

    // A matched path with the wrong method is a 405, not a fallback hit.
    let (status, _) = send(router.clone(), "POST", "/known", &[], Body::empty()).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

    // Everything else lands in the fallback, any method, with bean access.
    let (status, body) = send(router.clone(), "PUT", "/nope/nested", &[], Body::empty()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, "sorry: no route for PUT /nope/nested");
}

#[r2e_core::test]
async fn fallback_streams_and_redirects() {
    let router = fallback_router().await;

    let (status, body) = send(router.clone(), "GET", "/stream-me", &[], Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "part1|part2");

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/redirect-me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(resp.headers()["location"], "/known");
}

// ── 4. Guards and identity run on fallback routes ──────────────────────────

struct Subject(String);

impl Identity for Subject {
    fn sub(&self) -> &str {
        &self.0
    }
}

impl<S: Send + Sync> r2e_core::http::extract::FromRequestParts<S> for Subject {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get("x-user")
            .and_then(|v| v.to_str().ok())
            .map(|s| Subject(s.to_owned()))
            .ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())
    }
}

struct RecordingGuard;

struct RecordingGuardReady {
    saw: Arc<Mutex<Vec<String>>>,
}

impl r2e_core::DecoratorSpec for RecordingGuard {
    type Product = RecordingGuardReady;
    type Deps = r2e_core::type_list::TCons<Arc<Mutex<Vec<String>>>, r2e_core::type_list::TNil>;

    fn build(self, ctx: &r2e_core::BeanContext) -> RecordingGuardReady {
        RecordingGuardReady { saw: ctx.get() }
    }
}

impl Guard<Subject> for RecordingGuardReady {
    fn check(
        &self,
        ctx: &GuardContext<'_, Subject>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        let sub = ctx.identity.map(|i| i.sub().to_string());
        async move {
            if let Some(s) = sub {
                self.saw.lock().unwrap().push(s);
            }
            Ok(())
        }
    }
}

#[controller]
struct GuardedFallbackController {
    #[inject(identity)]
    user: Subject,
}

#[routes]
impl GuardedFallbackController {
    #[fallback]
    #[guard(RecordingGuard)]
    async fn catch_all(&self) -> String {
        format!("caught for {}", self.user.0)
    }
}

#[r2e_core::test]
async fn fallback_runs_identity_and_guards() {
    let saw: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let router = r2e_core::AppBuilder::new()
        .provide(saw.clone())
        .build_state()
        .await
        .register_controller::<GuardedFallbackController>()
        .build();

    // Identity extraction applies to the fallback: no header → 401.
    let (status, _) = send(router.clone(), "GET", "/whatever", &[], Body::empty()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = send(
        router,
        "GET",
        "/whatever",
        &[("x-user", "alice")],
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "caught for alice");
    assert_eq!(*saw.lock().unwrap(), vec!["alice"]);
}

// ── 5. NormalizePath composes with a controller fallback ───────────────────

#[r2e_core::test]
async fn normalize_path_composes_with_fallback() {
    let router = r2e_core::AppBuilder::new()
        .provide(Greeting("sorry".to_string()))
        .build_state()
        .await
        .register_controller::<FallbackController>()
        .with(r2e_core::plugins::NormalizePath)
        .build();

    // Trailing slash is normalized onto the declared route.
    let (status, body) = send(router.clone(), "GET", "/known/", &[], Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "known");

    // A plain miss still reaches the controller fallback...
    let (status, body) = send(router.clone(), "GET", "/missing", &[], Body::empty()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, "sorry: no route for GET /missing");

    // ...and so does a trailing-slash miss (normalized, then unmatched).
    let (status, body) = send(router, "GET", "/missing/", &[], Body::empty()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, "sorry: no route for GET /missing");
}

#[r2e_core::test]
async fn normalize_path_without_fallback_still_404s() {
    // No controller fallback: plain misses 404, trailing-slash misses
    // normalize (pre-routing rewrite) then 404.
    let router = r2e_core::AppBuilder::new()
        .with_state(())
        .with(r2e_core::plugins::NormalizePath)
        .build();

    let (status, _) = send(router.clone(), "GET", "/missing", &[], Body::empty()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(router, "GET", "/missing/", &[], Body::empty()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── 6. Interceptors wrap #[any] routes ──────────────────────────────────────

struct CountingInterceptor;

struct CountingReady {
    hits: Arc<AtomicUsize>,
}

impl r2e_core::DecoratorSpec for CountingInterceptor {
    type Product = CountingReady;
    type Deps = r2e_core::type_list::TCons<Arc<AtomicUsize>, r2e_core::type_list::TNil>;

    fn build(self, ctx: &r2e_core::BeanContext) -> CountingReady {
        CountingReady { hits: ctx.get() }
    }
}

impl<R: Send> Interceptor<R> for CountingReady {
    fn around<F, Fut>(&self, _ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        async move {
            self.hits.fetch_add(1, Ordering::SeqCst);
            next().await
        }
    }
}

#[controller]
struct InterceptedAnyController {}

#[routes]
impl InterceptedAnyController {
    #[any("/i/{*rest}")]
    #[intercept(CountingInterceptor)]
    async fn intercepted(&self) -> &'static str {
        "ok"
    }
}

#[r2e_core::test]
async fn any_route_runs_interceptors() {
    let hits: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let router = r2e_core::AppBuilder::new()
        .provide(hits.clone())
        .build_state()
        .await
        .register_controller::<InterceptedAnyController>()
        .build();

    for method in ["GET", "POST"] {
        let (status, body) = send(router.clone(), method, "/i/x", &[], Body::empty()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}
