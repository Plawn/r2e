//! Task #993 — one `AuthenticatedUser` per authenticated request.
//!
//! The auth layer deposits the caller's identity twice: once as the
//! `McpPrincipal` (scopes + token hash) and once as the identity extension the
//! `#[inject(identity)]` extractor reads. Both now point at the same
//! `Arc<AuthenticatedUser>`, so the second deposit is a refcount bump instead
//! of a second copy of the claims tree.
//!
//! The guard is a size invariant rather than an absolute count: the per-request
//! cost of a validated principal must not grow with how much the IdP put in the
//! token. The backend here returns a prebuilt principal — the shape of the
//! opaque-token cache hit (`auth::opaque`), where the whole per-request cost of
//! the identity IS the copying — so the only variable between the two
//! measurements is the size of the claims tree behind the `Arc`.

use std::future::Future;
use std::sync::Arc;

use r2e_core::http::Router;
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::auth::{DiscoveryMode, McpAuthConfig, McpAuthError, ScopePolicy, TokenValidatorBackend};
use r2e_mcp::{AppBuilderMcpExt, McpPrincipal, McpServer, McpTokenValidator};
use r2e_test::TestJwt;
use serde_json::json;

use crate::counter::{assert_config_size_invariant, runtime, steady_state, Alloc};
use crate::support;

const ISSUER: &str = "http://idp.test";
const RESOURCE: &str = "http://localhost:3000/mcp";
// ── Fixture service ────────────────────────────────────────────────────────

#[controller]
struct Guarded;

#[mcp_routes]
impl Guarded {
    /// A tool that reads nothing: the measurement is the request plumbing —
    /// validate, deposit the identity, dispatch — not the handler.
    #[tool]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

// ── A validator that returns a prebuilt principal ──────────────────────────

/// The cache-hit shape: validation is a clone of a principal built earlier.
struct Prebuilt(McpPrincipal);

impl TokenValidatorBackend for Prebuilt {
    fn validate(
        &self,
        _bearer: &str,
    ) -> impl Future<Output = Result<McpPrincipal, McpAuthError>> + Send {
        let principal = self.0.clone();
        async move { Ok(principal) }
    }
}

/// Mint a token carrying `claims` extra string claims of `size` bytes each,
/// then run it through a real validator to obtain a real principal (an
/// `AuthenticatedUser` is not literal-constructible).
async fn principal_with(claims: usize, size: usize) -> McpPrincipal {
    let jwt = TestJwt::with_config(b"hotpath-secret", ISSUER, RESOURCE);
    let mut builder = jwt.token_builder("alice").scopes(&["mcp:read"]);
    let blob = "x".repeat(size);
    for i in 0..claims {
        builder = builder.claim(&format!("attr_{i}"), blob.as_str());
    }
    let token = builder.build();
    McpTokenValidator::jwt(Arc::new(jwt.claims_validator()), ScopePolicy::default())
        .validate(&token)
        .await
        .expect("fixture token validates")
}

async fn app(principal: McpPrincipal) -> Router {
    let auth = McpAuthConfig {
        issuer: ISSUER.to_string(),
        resource: Some(RESOURCE.to_string()),
        discovery: Some(DiscoveryMode::Off),
        // A dead JWKS URL: an accidental fetch fails fast instead of
        // silently adding network allocations to the measurement.
        jwks_url: Some("http://127.0.0.1:1/jwks".to_string()),
        allow_insecure: Some(true),
        ..Default::default()
    };
    AppBuilder::new()
        .plugin(
            McpServer::new()
                .with_auth(auth)
                .with_token_validator(McpTokenValidator::custom(Prebuilt(principal))),
        )
        .build_state()
        .await
        .register_mcp_service::<Guarded>()
        .build()
}

/// The bearer the fixture backend is handed: a token whose *own* size cannot
/// move the measurement, so what is left is the principal it maps to.
fn bearer() -> [(&'static str, &'static str); 1] {
    [("authorization", "Bearer opaque-token")]
}

/// Boot, handshake, then measure `iterations` authenticated `tools/call`s.
fn cost_per_call(rt: &r2e_core::rt::Runtime, claims: usize, size: usize) -> Alloc {
    let router = rt.block_on(async {
        let principal = principal_with(claims, size).await;
        assert_eq!(principal.user.sub, "alice");
        app(principal).await
    });

    let session = rt.block_on(async {
        let response =
            support::post_with_headers(&router, "/mcp", None, &bearer(), &support::initialize_body())
                .await;
        let session = response.session_id.clone().expect("no Mcp-Session-Id");
        support::post_with_headers(
            &router,
            "/mcp",
            Some(&session),
            &bearer(),
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await;
        session
    });

    let body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "ping", "arguments": {} }
    });

    steady_state(20, || {
        let response = rt.block_on(support::post_with_headers(
            &router,
            "/mcp",
            Some(&session),
            &bearer(),
            &body,
        ));
        assert_eq!(response.result()["content"][0]["text"], "pong");
    })
}

/// A caller whose token carries 128 × 256-byte claims must cost the same per
/// request as one with none: the validated identity is shared, not copied.
#[test]
fn the_per_request_cost_does_not_scale_with_the_claims_tree() {
    let rt = runtime();
    let small = cost_per_call(&rt, 0, 0);
    let large = cost_per_call(&rt, 128, 256);

    // ~32 KiB of claims: a single extra copy of the tree would add ~128
    // allocations and ~40 KiB per request, two orders of magnitude past the
    // slack (which only absorbs a differently-sized format! buffer).
    assert_config_size_invariant(
        "authenticated MCP tools/call",
        small,
        large,
        4,
        2048,
    );
}

/// The structural reason: the principal's identity is behind an `Arc`, so the
/// clone the layer and the cache both perform is a refcount bump — no
/// allocation at all, whatever the token said.
#[test]
fn cloning_a_principal_allocates_nothing() {
    let rt = runtime();
    let principal = rt.block_on(principal_with(128, 256));

    drop(principal.clone());
    let (clone, alloc) = crate::counter::measure(|| principal.clone());
    drop(clone);

    eprintln!("[hotpath] McpPrincipal clone (128 x 256B claims): {alloc}");
    assert_eq!(
        alloc, Alloc::default(),
        "cloning an McpPrincipal allocated — the identity is held by value \
         instead of behind an Arc. See docs/claude/hot-path-clone-audit.md.",
    );
}
