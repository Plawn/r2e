//! Test helpers for authenticated MCP apps (feature `testing`).
//!
//! The no-Docker fast path: pin a [`McpTokenValidator`] built from a
//! [`TestJwt`] onto the builder so the app boots with **zero network I/O** —
//! no discovery fetch, no JWKS fetch — while the real auth layer, well-known
//! routes and per-tool scope checks stay active.
//!
//! ```ignore
//! let resource = "http://localhost:3000/mcp";
//! let jwt = TestJwt::for_resource(resource);
//! let app = pin_mcp_validator(AppBuilder::new(), &jwt, resource)
//!     .plugin(McpServer::new())
//!     .load_config()?
//!     .build_state()
//!     .await
//!     .register_mcp_service::<MathTools>();
//! let token = jwt.token_builder("alice").scopes(&["mcp:write"]).build();
//! ```

use std::sync::Arc;

use r2e_core::builder::NoState;
use r2e_core::AppBuilder;
use r2e_test::TestJwt;

use crate::auth::{McpTokenValidator, ScopePolicy};

/// Pin a [`TestJwt`]-backed token validator and the matching `mcp.auth.*`
/// config onto the builder.
///
/// What it does:
/// - `override_bean(McpTokenValidator::jwt(...))` — the pinned validator wins
///   over the one the plugin would build (the auth layer resolves its
///   validator from the bean context in `after_build`, so an
///   [`override_bean`](AppBuilder::override_bean) is picked up, not shadowed).
/// - `mcp.auth.issuer` / `mcp.auth.resource` — match the `TestJwt`'s claims
///   (call with `TestJwt::for_resource(resource)` so `aud` lines up).
/// - `mcp.auth.discovery: off` + a dummy `mcp.auth.jwks-url` — fixed
///   metadata, no discovery I/O. The plugin only sees validators passed via
///   `McpServer::with_token_validator` at build time (a pinned bean is
///   resolved later, in `after_build`), so the JWKS URL stays mandatory here;
///   it points at `127.0.0.1:1` so an accidental fetch fails instantly
///   instead of hanging.
/// - `mcp.auth.allow-insecure: true` — the `r2e-test` issuer is not an
///   `https://` URL.
///
/// Call **before** `load_config()` (config-value overrides must precede
/// typed-section construction). Roles/scopes on tokens minted by the same
/// `TestJwt` flow through the default [`ScopePolicy`] (`scope` claim,
/// `roles` + `realm_access.roles`).
pub fn pin_mcp_validator<P, R, Mods>(
    builder: AppBuilder<NoState, P, R, Mods>,
    jwt: &TestJwt,
    resource: &str,
) -> AppBuilder<NoState, P, R, Mods> {
    let validator =
        McpTokenValidator::jwt(Arc::new(jwt.claims_validator()), ScopePolicy::default());
    builder
        .override_bean(validator)
        .override_config_value("mcp.auth.issuer", jwt.issuer())
        .override_config_value("mcp.auth.resource", resource)
        .override_config_value("mcp.auth.discovery", "off")
        // Never fetched (the pinned bean replaces the lazy JWT backend), but
        // `discovery: off` + JWT validation requires it at build time.
        .override_config_value("mcp.auth.jwks-url", "http://127.0.0.1:1/jwks")
        .override_config_value("mcp.auth.allow-insecure", true)
}
