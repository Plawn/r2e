//! SPI #1: resolvers, and the router policy layered on top of them.
//!
//! The distinction that matters here is *absent* vs *malformed*: a request with
//! no tenant is `Ok(None)` and the deployment's policy decides what that means,
//! while a present-but-invalid tenant is the resolver's own 400 — a header
//! carrying `../etc/passwd` must never become "no tenant".

use std::net::SocketAddr;
use std::sync::Arc;

use r2e_core::http::{Extensions, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use r2e_core::web::request_head::RequestHead;
use r2e_core::{HttpError, PathParams};
use r2e_tenant::{
    ExtensionTenantResolver, FnTenantResolver, HeaderTenantResolver, MissingTenantPolicy,
    PathTenantResolver, Strict, SyncTenantResolver, TenantId, TenantResolver, TenantRouter,
    TenantStatuses,
};

/// A request head built from parts a test cares about.
struct Head {
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    extensions: Extensions,
}

impl Head {
    fn new(path: &str) -> Self {
        Self {
            method: Method::GET,
            uri: path.parse().unwrap(),
            headers: HeaderMap::new(),
            extensions: Extensions::new(),
        }
    }

    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
        self
    }

    fn extension<T: Clone + Send + Sync + 'static>(mut self, value: T) -> Self {
        self.extensions.insert(value);
        self
    }

    /// What the `Tenancy` layer does before routing: park the resolve-once
    /// cell, so every consumer of this head shares one resolver call.
    fn with_memo(mut self) -> Self {
        TenantRouter::install_memo(&mut self.extensions);
        self
    }

    fn head<'a>(&'a self, params: &'a [(&'a str, &'a str)]) -> RequestHead<'a> {
        RequestHead {
            method: &self.method,
            uri: &self.uri,
            headers: &self.headers,
            extensions: &self.extensions,
            path_params: PathParams::from_pairs(params),
            peer_addr: None,
        }
    }
}

/// Run a resolver over a head, awaiting through the object-safe SPI.
async fn resolve(
    resolver: &dyn TenantResolver,
    request: &Head,
    params: &[(&str, &str)],
) -> Result<Option<TenantId>, HttpError> {
    resolver.resolve(&request.head(params)).await
}

#[tokio::test]
async fn header_resolver_reads_the_default_header() {
    let resolver = HeaderTenantResolver::default();
    assert_eq!(resolver.header(), "x-tenant-id");

    let request = Head::new("/").header("x-tenant-id", "acme");
    let tenant = resolve(&resolver, &request, &[]).await.unwrap();
    assert_eq!(tenant.unwrap().as_str(), "acme");
}

#[tokio::test]
async fn header_resolver_is_case_insensitive_about_its_own_name() {
    // HTTP header names are case-insensitive; a resolver configured with
    // `X-Org-Id` must match the wire's `x-org-id`.
    let resolver = HeaderTenantResolver::new("X-Org-Id");
    assert_eq!(resolver.header(), "x-org-id");

    let request = Head::new("/").header("x-org-id", "acme");
    assert!(resolve(&resolver, &request, &[]).await.unwrap().is_some());
}

#[tokio::test]
async fn an_absent_header_is_not_an_error() {
    let resolver = HeaderTenantResolver::default();
    let request = Head::new("/");
    assert_eq!(resolve(&resolver, &request, &[]).await.unwrap(), None);
}

#[tokio::test]
async fn a_malformed_header_is_a_400_naming_the_header() {
    let resolver = HeaderTenantResolver::default();
    for hostile in ["../etc/passwd", "Acme", "", "a b"] {
        let request = Head::new("/").header("x-tenant-id", hostile);
        let err = resolve(&resolver, &request, &[])
            .await
            .expect_err("a present-but-invalid tenant must not be treated as absent");
        assert_eq!(err.status(), StatusCode::BAD_REQUEST, "for `{hostile}`");
        assert!(
            err.to_string().contains("x-tenant-id"),
            "the message must name the header: {err}"
        );
    }
}

#[tokio::test]
async fn path_resolver_reads_the_matched_parameter() {
    let resolver = PathTenantResolver::default();
    assert_eq!(resolver.param(), "tenant");

    let request = Head::new("/t/acme/orders");
    let tenant = resolve(&resolver, &request, &[("tenant", "acme")])
        .await
        .unwrap();
    assert_eq!(tenant.unwrap().as_str(), "acme");

    // A route that does not carry the parameter simply has no tenant.
    assert_eq!(resolve(&resolver, &request, &[]).await.unwrap(), None);
}

#[tokio::test]
async fn a_malformed_path_parameter_is_a_400() {
    let resolver = PathTenantResolver::new("org");
    let request = Head::new("/o/BAD/x");
    let err = resolve(&resolver, &request, &[("org", "BAD")])
        .await
        .unwrap_err();
    assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    assert!(err.to_string().contains("{org}"), "{err}");
}

#[tokio::test]
async fn extension_resolver_projects_what_upstream_already_parsed() {
    // The documented JWT pattern: whoever validated the token parks a claim, the
    // resolver projects it — no second parse.
    #[derive(Clone)]
    struct TenantClaim(String);

    let resolver = ExtensionTenantResolver::<TenantClaim, _>::new(|claim: &TenantClaim| {
        TenantId::parse(&claim.0).ok()
    });

    let request = Head::new("/").extension(TenantClaim("acme".into()));
    assert_eq!(
        resolve(&resolver, &request, &[])
            .await
            .unwrap()
            .unwrap()
            .as_str(),
        "acme"
    );

    // No claim at all, and a claim that is not a valid id, both mean "no tenant"
    // — the projection returns `Option`, so it cannot invent a 400.
    assert_eq!(resolve(&resolver, &Head::new("/"), &[]).await.unwrap(), None);
    let bad = Head::new("/").extension(TenantClaim("../x".into()));
    assert_eq!(resolve(&resolver, &bad, &[]).await.unwrap(), None);
}

#[tokio::test]
async fn a_strict_extension_resolver_can_reject_a_malformed_claim() {
    // The other half of the same pattern: a claim that is *present but wrong* is
    // a client error, not "no tenant". The lenient projection cannot say so, so
    // `try_new` takes a fallible one.
    #[derive(Clone)]
    struct TenantClaim(String);

    let resolver = ExtensionTenantResolver::<TenantClaim, _, Strict>::try_new(
        |claim: &TenantClaim| {
            TenantId::parse(&claim.0)
                .map(Some)
                .map_err(|err| HttpError::bad_request(format!("bad tenant claim: {err}")))
        },
    );

    let good = Head::new("/").extension(TenantClaim("acme".into()));
    assert_eq!(
        resolve(&resolver, &good, &[]).await.unwrap().unwrap().as_str(),
        "acme"
    );

    // No claim is still "no tenant" — the missing-tenant policy decides, not the
    // resolver.
    assert_eq!(resolve(&resolver, &Head::new("/"), &[]).await.unwrap(), None);

    let bad = Head::new("/").extension(TenantClaim("../x".into()));
    let err = resolve(&resolver, &bad, &[]).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    assert!(err.to_string().contains("bad tenant claim"), "{err}");
}

#[tokio::test]
async fn fn_resolver_wraps_a_closure() {
    let resolver = FnTenantResolver::new(|req: &RequestHead<'_>| {
        Ok(req.header("x-org").and_then(|raw| TenantId::parse(raw).ok()))
    });
    let request = Head::new("/").header("x-org", "acme");
    assert!(resolve(&resolver, &request, &[]).await.unwrap().is_some());
}

#[tokio::test]
async fn a_custom_sync_resolver_gets_the_async_spi_for_free() {
    // The blanket bridge: implementing `SyncTenantResolver` is enough to be used
    // as `Arc<dyn TenantResolver>`.
    #[derive(Clone)]
    struct SubdomainResolver;

    impl SyncTenantResolver for SubdomainResolver {
        fn resolve_sync(&self, req: &RequestHead<'_>) -> Result<Option<TenantId>, HttpError> {
            let Some(host) = req.host() else {
                return Ok(None);
            };
            let Some((label, _)) = host.split_once('.') else {
                return Ok(None);
            };
            TenantId::parse(label)
                .map(Some)
                .map_err(|e| HttpError::bad_request(format!("invalid tenant subdomain: {e}")))
        }
    }

    let boxed: Arc<dyn TenantResolver> = Arc::new(SubdomainResolver);
    let request = Head::new("/").header("host", "acme.example.com");
    assert_eq!(
        boxed
            .resolve(&request.head(&[]))
            .await
            .unwrap()
            .unwrap()
            .as_str(),
        "acme"
    );

    // A bare host with no subdomain carries no tenant.
    let request = Head::new("/").header("host", "example");
    assert_eq!(boxed.resolve(&request.head(&[])).await.unwrap(), None);
}

#[tokio::test]
async fn an_async_resolver_can_do_io() {
    // The reason the SPI is async at all: a directory lookup.
    struct DirectoryResolver;

    impl TenantResolver for DirectoryResolver {
        fn resolve<'a>(
            &'a self,
            req: &'a RequestHead<'a>,
        ) -> r2e_tenant::BoxFuture<'a, Result<Option<TenantId>, HttpError>> {
            Box::pin(async move {
                let Some(key) = req.header("x-api-key") else {
                    return Ok(None);
                };
                tokio::task::yield_now().await;
                match key {
                    "key-1" => Ok(Some(TenantId::parse("acme").unwrap())),
                    _ => Err(HttpError::from_status(
                        StatusCode::UNAUTHORIZED,
                        "unknown api key",
                    )),
                }
            })
        }
    }

    let request = Head::new("/").header("x-api-key", "key-1");
    assert!(resolve(&DirectoryResolver, &request, &[])
        .await
        .unwrap()
        .is_some());

    let request = Head::new("/").header("x-api-key", "nope");
    assert_eq!(
        resolve(&DirectoryResolver, &request, &[])
            .await
            .unwrap_err()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

// ── the router's policy layer ───────────────────────────────────────────────

fn router(policy: MissingTenantPolicy) -> TenantRouter {
    TenantRouter::ready(
        Arc::new(HeaderTenantResolver::default()),
        policy,
        TenantStatuses::default(),
    )
}

#[tokio::test]
async fn reject_policy_turns_a_missing_tenant_into_the_missing_status() {
    let router = router(MissingTenantPolicy::Reject);
    let request = Head::new("/");
    let err = router.try_resolve(&request.head(&[])).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    assert!(err.to_string().contains("no tenant"), "{err}");
}

#[tokio::test]
async fn allow_policy_lets_a_missing_tenant_through_as_none() {
    let router = router(MissingTenantPolicy::Allow);
    let request = Head::new("/");
    assert_eq!(router.try_resolve(&request.head(&[])).await.unwrap(), None);

    // `resolve` (the non-optional form) still fails — the policy governs whether
    // *absence* is legal, not whether a required resource can do without one.
    assert_eq!(
        router.resolve(&request.head(&[])).await.unwrap_err().status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn the_missing_status_is_configurable() {
    let router = TenantRouter::ready(
        Arc::new(HeaderTenantResolver::default()),
        MissingTenantPolicy::Reject,
        TenantStatuses {
            missing: StatusCode::UNAUTHORIZED,
            ..TenantStatuses::default()
        },
    );
    let request = Head::new("/");
    assert_eq!(
        router
            .try_resolve(&request.head(&[]))
            .await
            .unwrap_err()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_disabled_router_resolves_nothing_without_failing() {
    // `tenancy.enabled = false`: the app boots and `Option` extractors see `None`.
    let router = TenantRouter::disabled(TenantStatuses::default());
    assert!(!router.is_enabled());
    assert_eq!(router.policy(), MissingTenantPolicy::Allow);

    let request = Head::new("/").header("x-tenant-id", "acme");
    assert_eq!(router.try_resolve(&request.head(&[])).await.unwrap(), None);
}

/// A resolver that counts its calls and answers a different tenant every time.
///
/// The adversarial shape from the audit: if anything resolves twice in one
/// request, the two answers disagree and the test can see it.
#[derive(Clone)]
struct Alternating {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    answers: Vec<&'static str>,
}

impl Alternating {
    fn new(answers: &[&'static str]) -> Self {
        Self {
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            answers: answers.to_vec(),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl SyncTenantResolver for Alternating {
    fn resolve_sync(&self, _req: &RequestHead<'_>) -> Result<Option<TenantId>, HttpError> {
        let nth = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Some(
            TenantId::parse(self.answers[nth % self.answers.len()]).unwrap(),
        ))
    }
}

#[tokio::test]
async fn the_resolve_once_cell_short_circuits_the_resolver() {
    // The whole point of the cell: whatever asks — a guard, an extractor, two
    // `#[managed]` resources — the resolver runs once and everybody gets the
    // same tenant, even when the resolver itself is not deterministic.
    let resolver = Alternating::new(&["acme", "globex"]);
    let router = TenantRouter::ready(
        Arc::new(resolver.clone()),
        MissingTenantPolicy::Reject,
        TenantStatuses::default(),
    );

    let request = Head::new("/").with_memo();
    let head = request.head(&[]);
    for _ in 0..3 {
        assert_eq!(
            router.try_resolve(&head).await.unwrap().unwrap().as_str(),
            "acme"
        );
    }
    assert_eq!(resolver.calls(), 1, "the resolver must run once per request");
    assert_eq!(
        TenantRouter::memoized(&head).map(TenantId::as_str),
        Some("acme")
    );
}

#[tokio::test]
async fn without_the_cell_every_caller_resolves_for_itself() {
    // The degraded path (a hand-wired router with no `Tenancy` layer): worth
    // pinning, because it is exactly what the cell exists to prevent.
    let resolver = Alternating::new(&["acme", "globex"]);
    let router = TenantRouter::ready(
        Arc::new(resolver.clone()),
        MissingTenantPolicy::Reject,
        TenantStatuses::default(),
    );

    let request = Head::new("/");
    let head = request.head(&[]);
    assert_eq!(
        router.try_resolve(&head).await.unwrap().unwrap().as_str(),
        "acme"
    );
    assert_eq!(
        router.try_resolve(&head).await.unwrap().unwrap().as_str(),
        "globex"
    );
    assert_eq!(resolver.calls(), 2);
    assert_eq!(TenantRouter::memoized(&head), None);
}

#[tokio::test]
async fn a_raw_tenant_id_extension_is_not_the_memo() {
    // Finding 9: the memo carrier is private. A `TenantId` some middleware parks
    // in the extensions for its own purposes must NOT override the configured
    // resolver — a request whose header says `acme` is served as `acme`.
    let router = TenantRouter::ready(
        Arc::new(HeaderTenantResolver::default()),
        MissingTenantPolicy::Reject,
        TenantStatuses::default(),
    );

    let request = Head::new("/")
        .header("x-tenant-id", "acme")
        .extension(TenantId::parse("attacker").unwrap())
        .with_memo();
    let head = request.head(&[]);
    assert_eq!(
        router.try_resolve(&head).await.unwrap().unwrap().as_str(),
        "acme"
    );
    assert_eq!(
        TenantRouter::memoized(&head).map(TenantId::as_str),
        Some("acme")
    );
}

#[tokio::test]
async fn the_absence_of_a_tenant_is_memoized_too() {
    // The cell holds the resolver's own answer, `None` included, so a
    // tenant-less request does not re-run the resolver per consumer either. The
    // *policy* is applied per call site, which is why the same cell serves an
    // `Option` extractor and a required one.
    let resolver = Alternating::new(&["acme"]);
    let calls = resolver.calls.clone();
    let router = TenantRouter::ready(
        Arc::new(FnTenantResolver::new(move |_: &RequestHead<'_>| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        })),
        MissingTenantPolicy::Allow,
        TenantStatuses::default(),
    );

    let request = Head::new("/").with_memo();
    let head = request.head(&[]);
    assert_eq!(router.try_resolve(&head).await.unwrap(), None);
    assert_eq!(router.try_resolve(&head).await.unwrap(), None);
    assert_eq!(resolver.calls(), 1);
}

#[tokio::test]
async fn a_resolver_error_is_not_memoized() {
    // A failing resolve leaves the cell empty: the request is about to end with
    // that error anyway, and caching it would leak into a retry that shares the
    // head (there is none today, but the semantics are the honest ones).
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = attempts.clone();
    let router = TenantRouter::ready(
        Arc::new(FnTenantResolver::new(move |_: &RequestHead<'_>| {
            let nth = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if nth == 0 {
                Err(HttpError::bad_request("boom"))
            } else {
                Ok(Some(TenantId::parse("acme").unwrap()))
            }
        })),
        MissingTenantPolicy::Reject,
        TenantStatuses::default(),
    );

    let request = Head::new("/").with_memo();
    let head = request.head(&[]);
    assert_eq!(
        router.try_resolve(&head).await.unwrap_err().status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        router.try_resolve(&head).await.unwrap().unwrap().as_str(),
        "acme"
    );
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn resolver_errors_pass_through_untouched() {
    // A resolver owns its own status: the router must not relabel a 403 as a 400.
    let router = TenantRouter::ready(
        Arc::new(FnTenantResolver::new(|_: &RequestHead<'_>| {
            Err(HttpError::from_status(
                StatusCode::FORBIDDEN,
                "tenant suspended",
            ))
        })),
        MissingTenantPolicy::Allow,
        TenantStatuses::default(),
    );
    let request = Head::new("/");
    let err = router.try_resolve(&request.head(&[])).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::FORBIDDEN);
    assert!(err.to_string().contains("suspended"), "{err}");
}

#[tokio::test]
async fn the_head_exposes_the_peer_address_to_resolvers() {
    // Not a resolver R2E ships, but the SPI must be able to see the peer — a
    // gateway-per-tenant deployment resolves on it.
    let resolver = FnTenantResolver::new(|req: &RequestHead<'_>| {
        Ok(req
            .peer_addr
            .filter(|addr: &SocketAddr| addr.port() == 8443)
            .map(|_| TenantId::parse("gateway").unwrap()))
    });
    let request = Head::new("/");
    let mut head = request.head(&[]);
    head.peer_addr = Some("127.0.0.1:8443".parse().unwrap());
    assert_eq!(
        resolver.resolve(&head).await.unwrap().unwrap().as_str(),
        "gateway"
    );
}
