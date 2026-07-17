// Canonical example-oidc application source.
//
// `lib.rs` includes this file so the app can be booted by type; `app_main!`
// includes the same file directly in the binary tip crate for production and
// real Subsecond hot-patching.

use r2e::prelude::*;
use r2e::r2e_oidc::{ClientRegistry, InMemoryUserStore, OidcRuntime, OidcServer, OidcUser};
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};
use r2e::r2e_security::AuthenticatedUser;
use serde::Serialize;

// ---------------------------------------------------------------------------
// Controller
// ---------------------------------------------------------------------------

#[controller(path = "/")]
pub struct GreetingController;

#[routes]
impl GreetingController {
    /// Public endpoint — no authentication required.
    #[get("/public")]
    async fn public_hello(&self) -> Json<Message> {
        Json(Message {
            message: "Hello, world! This endpoint is public.".into(),
        })
    }

    /// Returns the authenticated user's identity.
    #[get("/me")]
    async fn me(
        &self,
        #[inject(identity)] user: AuthenticatedUser,
    ) -> Json<AuthenticatedUser> {
        Json(user)
    }

    /// Admin-only endpoint.
    #[get("/admin")]
    #[roles("admin")]
    async fn admin(
        &self,
        #[inject(identity)] user: AuthenticatedUser,
    ) -> Json<Message> {
        Json(Message {
            message: format!("Hello admin {}!", user.sub),
        })
    }
}

#[derive(Serialize)]
struct Message {
    message: String,
}

// ---------------------------------------------------------------------------
// Long-lived environment
// ---------------------------------------------------------------------------

/// Resources provisioned once by [`App::setup`]. In dev mode they survive
/// hot-patches (only [`App::build`] re-runs per patch).
#[derive(Clone)]
pub struct AppEnv {
    oidc: OidcRuntime,
}

// ---------------------------------------------------------------------------
// Application blueprint
// ---------------------------------------------------------------------------

/// The canonical application blueprint. Production (`r2e::app_main!(OidcApp)`),
/// dev hot-reload, and tests all go through this single [`App`] impl:
/// [`App::setup`] builds the long-lived environment once, [`App::build`]
/// assembles the app on a fresh builder per patch.
pub struct OidcApp;

impl App for OidcApp {
    /// Long-lived resources; in dev mode they survive hot-patches.
    type Env = AppEnv;

    /// Called once per process — expensive setup that survives hot-patches.
    async fn setup() -> AppEnv {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "info,r2e=debug".parse().unwrap()),
            )
            .init();

        // -- User store ------------------------------------------------------
        let users = InMemoryUserStore::new()
            .add_user(
                "alice",
                "password123",
                OidcUser {
                    sub: "user-1".into(),
                    email: Some("alice@example.com".into()),
                    roles: vec!["admin".into(), "user".into()],
                    ..Default::default()
                },
            )
            .add_user(
                "bob",
                "password123",
                OidcUser {
                    sub: "user-2".into(),
                    email: Some("bob@example.com".into()),
                    roles: vec!["user".into()],
                    ..Default::default()
                },
            );

        // -- Client registry (for client_credentials grant) -----------------
        let clients = ClientRegistry::new().add_client("my-service", "service-secret");

        // -- Build OidcRuntime (RSA keygen happens here, once) ---------------
        let oidc = OidcServer::new()
            .issuer("http://localhost:3000")
            .audience("r2e-app")
            .with_user_store(users)
            .with_client_registry(clients)
            .build();

        println!("=== example-oidc ready on http://localhost:3000 ===");
        println!("Try:");
        println!("  curl -s -X POST localhost:3000/oauth/token -d 'grant_type=password&username=alice&password=password123' | jq");
        println!("  curl -s localhost:3000/public | jq");
        println!("  curl -s localhost:3000/me -H 'Authorization: Bearer <token>' | jq");
        println!("  curl -s localhost:3000/admin -H 'Authorization: Bearer <token>' | jq");
        println!("  curl -s -X POST localhost:3000/oauth/token -d 'grant_type=client_credentials&client_id=my-service&client_secret=service-secret' | jq");

        AppEnv { oidc }
    }

    /// Re-run on every hot-patch: assemble the app on the given builder.
    async fn build(b: AppBuilder, env: AppEnv) -> impl BootableApp {
        b.plugin(env.oidc)
            .build_state()
            .await
            .with(Health)
            .with(Cors::permissive())
            .with(Tracing)
            .with(ErrorHandling)
            .with(OpenApiPlugin::new(
                OpenApiConfig::new("Example OIDC API", "0.1.0")
                    .with_description("Embedded OIDC server")
                    .with_docs_ui(true),
            ))
            .register_controller::<GreetingController>()
    }
}
