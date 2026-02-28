use std::sync::Arc;

use r2e::prelude::*;
use r2e::r2e_oidc::{ClientRegistry, InMemoryUserStore, OidcRuntime, OidcServer, OidcUser};
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};
use r2e::r2e_security::{AuthenticatedUser, JwtClaimsValidator};
use serde::Serialize;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone, BeanState)]
struct Services {
    claims_validator: Arc<JwtClaimsValidator>,
}

// ---------------------------------------------------------------------------
// Controller
// ---------------------------------------------------------------------------

#[derive(Controller)]
#[controller(path = "/", state = Services)]
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
// Hot-reload environment
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppEnv {
    oidc: OidcRuntime,
    listener: Arc<std::net::TcpListener>,
}

/// Called once — expensive setup that survives across hot-reload cycles.
async fn setup() -> AppEnv {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,r2e=debug".parse().unwrap()),
        )
        .init();

    // -- User store ----------------------------------------------------------
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

    // -- Client registry (for client_credentials grant) ----------------------
    let clients = ClientRegistry::new().add_client("my-service", "service-secret");

    // -- Build OidcRuntime (RSA keygen happens here, once) --------------------
    let oidc = OidcServer::new()
        .issuer("http://localhost:3000")
        .audience("r2e-app")
        .with_user_store(users)
        .with_client_registry(clients)
        .build();

    // -- Bind listener once ---------------------------------------------------
    let listener = std::net::TcpListener::bind("0.0.0.0:3000").unwrap();
    listener.set_nonblocking(true).unwrap();

    println!("=== example-oidc ready on http://localhost:3000 ===");
    println!("Try:");
    println!("  curl -s -X POST localhost:3000/oauth/token -d 'grant_type=password&username=alice&password=password123' | jq");
    println!("  curl -s localhost:3000/public | jq");
    println!("  curl -s localhost:3000/me -H 'Authorization: Bearer <token>' | jq");
    println!("  curl -s localhost:3000/admin -H 'Authorization: Bearer <token>' | jq");
    println!("  curl -s -X POST localhost:3000/oauth/token -d 'grant_type=client_credentials&client_id=my-service&client_secret=service-secret' | jq");

    AppEnv {
        oidc,
        listener: Arc::new(listener),
    }
}

// ---------------------------------------------------------------------------
// Server — hot-patched on each code change when dev-reload is enabled
// ---------------------------------------------------------------------------

#[r2e::main]
async fn main(env: AppEnv) {
    AppBuilder::new()
        .plugin(env.oidc.clone())
        .build_state::<Services, _, _>()
        .await
        .with(Health)
        .with(Cors::permissive())
        .with(Tracing)
        .with(ErrorHandling)
        .with(DevReload)
        .with(OpenApiPlugin::new(
            OpenApiConfig::new("Example OIDC API", "0.1.0")
                .with_description("Embedded OIDC server with hot-reload support")
                .with_docs_ui(true),
        ))
        .register_controller::<GreetingController>()
        .prepare("0.0.0.0:3000")
        .run_with_listener(
            tokio::net::TcpListener::from_std(env.listener.try_clone().unwrap()).unwrap(),
        )
        .await
        .inspect_err(|e| eprintln!("=== SERVE ERROR: {e} ==="))
        .unwrap();
}
