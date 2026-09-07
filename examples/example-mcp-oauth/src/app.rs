// Minimal authenticated MCP server: ONE tool behind a real OAuth 2.1
// resource server (Keycloak), reachable from Claude Code / claude.ai /
// ChatGPT through an ngrok tunnel.
//
// Everything auth-related is configuration (`application.yaml`), not code:
// `mcp.auth.issuer` + `server.public-url` turn the endpoint into a
// spec-compliant resource server (JWKS validation, RFC 9728
// protected-resource metadata, `WWW-Authenticate` challenges, DCR shim).
//
// `lib.rs` includes this file so the app can be booted by type; `app_main!`
// compiles the same file into the binary tip crate.

use r2e::prelude::*;
use schemars::JsonSchema;
use serde::Serialize;

// ── Tool result ────────────────────────────────────────────────────────
//
// `Json<T: Serialize + JsonSchema>` is dual-encoded on the wire
// (`structuredContent` + a JSON text block) and advertises an
// `outputSchema`, so the agent gets a typed result instead of a blob.

#[derive(Serialize, JsonSchema)]
pub struct WhoAmI {
    /// The `sub` claim: the caller's stable user id.
    pub sub: String,
    /// Keycloak's `preferred_username`, when the token carries one.
    pub username: Option<String>,
    /// The `email` claim, when present.
    pub email: Option<String>,
    /// Realm + client roles, as extracted by the default role extractor.
    pub roles: Vec<String>,
    /// The scopes granted to this access token.
    pub scopes: Vec<String>,
    /// The issuer that minted the token.
    pub issuer: Option<String>,
}

// ── The one tool ───────────────────────────────────────────────────────
//
// A unit controller: no beans, no config fields. `#[inject(identity)]` on a
// tool parameter reads the principal the MCP auth layer validated. Being
// REQUIRED (not `Option<_>`), an unauthenticated call is a JSON-RPC
// `unauthorized` error — the whole point of this smoke test.

#[controller]
pub struct IdentityTools;

#[mcp_routes]
impl IdentityTools {
    /// Report who the calling agent is authenticated as.
    ///
    /// Returns the subject, username, email, roles and scopes carried by the
    /// OAuth access token the MCP client presented.
    #[tool(read_only, idempotent)]
    async fn whoami(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<WhoAmI> {
        Json(WhoAmI {
            sub: user.sub.clone(),
            username: user.claims.preferred_username.clone(),
            email: user.email.clone(),
            roles: user.roles.clone(),
            scopes: user
                .claims
                .scope
                .as_deref()
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            issuer: user.claims.iss.clone(),
        })
    }
}

// ── Application blueprint ──────────────────────────────────────────────

pub struct McpOAuthApp;

impl App for McpOAuthApp {
    type Env = ();

    async fn setup() -> Result<(), BootError> {
        Ok(())
    }

    async fn build(b: AppBuilder, _env: ()) -> Result<impl BootableApp, BootError> {
        Ok(b
            // Reads `application.yaml` next to the crate, the profile overlay
            // and `R2E_*` env vars. The `mcp.auth` section is what makes the
            // endpoint authenticated — delete it and the server goes public.
            .load_config::<()>()
            .plugin(McpServer::new())
            .build_state()
            .await
            .on_start(|_state| async move {
                tracing::info!("MCP endpoint mounted at /mcp — expose it with ngrok");
                Ok(())
            })
            .register_mcp_service::<IdentityTools>())
    }
}
