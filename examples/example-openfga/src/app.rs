// Canonical example-openfga application source.
//
// `lib.rs` includes this file so the app can be booted by type; `app_main!`
// includes the same file directly in the binary tip crate for production and
// real Subsecond hot-patching.

use std::sync::Arc;

use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
use r2e::prelude::*;
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};
use r2e::r2e_openfga::{FgaClient, GrpcBackend, OpenFgaConfig, OpenFgaRegistry};
use r2e::r2e_security::{JwtClaimsValidator, SecurityConfig};

pub mod controllers;

use controllers::document_controller::DocumentController;

// Typed authorization API generated from the checked-in model: `authz::MODEL`
// (the schema 1.1 JSON) plus compile-checked markers — a typo in
// `authz::document::viewer` is a build error, not a prod 403.
r2e::r2e_openfga::model!(pub mod authz = "fga/model.fga");

/// HS256 secret for the demo validator. Tests replace the validator entirely
/// via the `#[r2e::test]` / `TestApp::boot` harness, so this only matters for
/// standalone `cargo run` + the demo token below.
const DEMO_SECRET: &[u8] = b"r2e-openfga-demo-secret-change-in-production";
const DEMO_ISSUER: &str = "r2e-openfga-demo";
const DEMO_AUDIENCE: &str = "r2e-openfga-app";

/// The document authorization model, straight from `fga/model.fga` via the
/// `model!`-generated `authz::MODEL` — code and store share one source of
/// truth. Exposed so integration tests write the exact model the guards are
/// compile-checked against.
pub fn document_model() -> serde_json::Value {
    serde_json::from_str(authz::MODEL).expect("model! output is valid JSON")
}

/// Mint a demo HS256 JWT accepted by the app's own validator, for `curl` usage
/// against a `cargo run` instance (subject `alice`).
pub fn demo_token(subject: &str) -> String {
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    let claims = serde_json::json!({
        "sub": subject,
        "iss": DEMO_ISSUER,
        "aud": DEMO_AUDIENCE,
        "exp": exp,
    });
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(DEMO_SECRET),
    )
    .unwrap()
}

fn demo_validator() -> Arc<JwtClaimsValidator> {
    let config = SecurityConfig::new("unused", DEMO_ISSUER, DEMO_AUDIENCE)
        .with_allowed_algorithm(Algorithm::HS256);
    Arc::new(JwtClaimsValidator::new_with_static_key(
        DecodingKey::from_secret(DEMO_SECRET),
        config,
    ))
}

/// The gRPC OpenFGA client bean. Connects at build time using the
/// `openfga.endpoint` / `openfga.store_id` / `openfga.model_id` config keys.
#[producer]
async fn openfga_backend(
    #[config("openfga.endpoint")] endpoint: String,
    #[config("openfga.store_id")] store_id: String,
    #[config("openfga.model_id")] model_id: Option<String>,
) -> GrpcBackend {
    let mut config = OpenFgaConfig::new(endpoint, store_id);
    if let Some(model_id) = model_id {
        config = config.with_model_id(model_id);
    }
    GrpcBackend::connect(&config)
        .await
        .expect("failed to connect to OpenFGA — is the server reachable and the store created?")
}

/// The cached authorization registry the `FgaCheck` guards resolve from the
/// bean graph at controller registration.
#[producer]
async fn openfga_registry(backend: GrpcBackend) -> OpenFgaRegistry {
    OpenFgaRegistry::with_cache(backend, 60)
}

/// The typed client for handler-level FGA operations: `grant`/`revoke`
/// (compile-checked subjects + write-through cache invalidation) and
/// `check`.
#[producer]
async fn openfga_client(registry: OpenFgaRegistry) -> FgaClient {
    FgaClient::new(registry)
}

/// The canonical application blueprint.
pub struct OpenFgaApp;

impl App for OpenFgaApp {
    type Env = ();

    async fn setup() {}

    async fn build(b: AppBuilder, _env: ()) -> impl BootableApp {
        b.load_config::<()>()
            .provide(demo_validator())
            .register::<OpenfgaBackend>()
            .register::<OpenfgaRegistry>()
            .register::<OpenfgaClient>()
            .build_state()
            .await
            .with(Health)
            .with(Cors::permissive())
            .with(Tracing)
            .with(ErrorHandling)
            .with(OpenApiPlugin::new(
                OpenApiConfig::new("Documents API", "1.0.0")
                    .with_description("OpenFGA fine-grained authorization example")
                    .with_docs_ui(true),
            ))
            .register_controller::<DocumentController>()
    }
}
