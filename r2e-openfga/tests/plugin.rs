//! Tests for the `OpenFga` plugin: install-time config validation (offline)
//! and the boot-time store/model lifecycle (against a real OpenFGA server via
//! `DevOpenFga` — requires Docker, `#[ignore]`d by default):
//!
//! ```bash
//! cargo test -p r2e-openfga --test plugin -- --ignored
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

use r2e_core::{AppBuilder, BeanAccess, R2eConfig};
use r2e_devservices::DevOpenFga;
use r2e_openfga::{FgaClient, OpenFga, OpenFgaHandle, OpenFgaRegistry};

r2e_openfga::model!(
    pub mod authz = inline "
model
  schema 1.1

type user

type document
  relations
    define viewer: [user]
    define editor: [user]
"
);

/// A structurally different model (extra relation) for mismatch tests.
r2e_openfga::model!(
    pub mod authz_v2 = inline "
model
  schema 1.1

type user

type document
  relations
    define viewer: [user]
    define editor: [user]
    define owner: [user]
"
);

fn yaml(endpoint: &str, store: &str, extra: &str) -> R2eConfig {
    R2eConfig::from_yaml_str(&format!(
        "openfga:\n  endpoint: \"{endpoint}\"\n  store: \"{store}\"\n{extra}"
    ))
    .unwrap()
}

/// Unique store name per test invocation, so tests sharing the
/// workspace-session OpenFGA container never collide.
fn unique_store(tag: &str) -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    format!(
        "r2e-plugin-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

// ── Install-time config validation (offline) ───────────────────────────

#[r2e_core::test]
#[should_panic(expected = "openfga.endpoint")]
async fn install_panics_without_endpoint() {
    let config = R2eConfig::from_yaml_str("openfga:\n  store: \"x\"\n").unwrap();
    let _ = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(OpenFga::model(authz::MODEL));
}

#[r2e_core::test]
#[should_panic(expected = "openfga.store")]
async fn install_panics_without_store() {
    let config =
        R2eConfig::from_yaml_str("openfga:\n  endpoint: \"http://localhost:1\"\n").unwrap();
    let _ = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(OpenFga::model(authz::MODEL));
}

#[r2e_core::test]
#[should_panic(expected = "verify mode")]
async fn install_panics_on_model_id_in_apply_mode() {
    let config = R2eConfig::from_yaml_str(
        "openfga:\n  endpoint: \"http://localhost:1\"\n  store: \"x\"\n  model_id: \"m\"\n",
    )
    .unwrap();
    let _ = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(OpenFga::model(authz::MODEL));
}

#[r2e_core::test]
#[should_panic(expected = "load_config")]
async fn install_panics_without_loaded_config() {
    let _ = AppBuilder::new().plugin(OpenFga::model(authz::MODEL));
}

/// `enabled: false` is a complete off-switch: no connection, no config
/// validation (no endpoint/store needed), and checks fail closed.
#[r2e_core::test]
async fn disabled_plugin_boots_offline_and_fails_closed() {
    let config = R2eConfig::from_yaml_str("openfga:\n  enabled: false\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(OpenFga::model(authz::MODEL))
        .build_state()
        .await;
    let state = app.state();

    let handle = state.get::<OpenFgaHandle>();
    assert!(handle.try_backend().is_none());

    let registry = state.get::<OpenFgaRegistry>();
    let err = registry
        .check("user:alice", "viewer", "document:readme")
        .await
        .expect_err("disabled plugin must fail closed");
    assert!(matches!(err, r2e_openfga::OpenFgaError::NotReady));
}

// ── Boot lifecycle (Docker) ────────────────────────────────────────────

async fn boot(config: R2eConfig) -> (OpenFgaHandle, OpenFgaRegistry, FgaClient) {
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(OpenFga::model(authz::MODEL))
        .build_state()
        .await;
    let state = app.state();
    (
        state.get::<OpenFgaHandle>(),
        state.get::<OpenFgaRegistry>(),
        state.get::<FgaClient>(),
    )
}

#[r2e_core::test]
#[ignore = "requires Docker"]
async fn boot_creates_store_and_applies_model() {
    let fga = DevOpenFga::shared().await;
    let store = unique_store("create");
    let (handle, registry, _) = boot(yaml(fga.grpc_endpoint(), &store, "")).await;

    // Store + model exist and are pinned on the handle.
    assert!(!handle.store_id().is_empty());
    assert!(!handle.model_id().is_empty());

    // The registry is live (a check against the fresh store runs and denies).
    let allowed = registry
        .check("user:alice", "viewer", "document:readme")
        .await
        .unwrap();
    assert!(!allowed);
}

#[r2e_core::test]
#[ignore = "requires Docker"]
async fn second_boot_reuses_identical_model() {
    let fga = DevOpenFga::shared().await;
    let store = unique_store("reuse");
    let (h1, _, _) = boot(yaml(fga.grpc_endpoint(), &store, "")).await;
    let (h2, _, _) = boot(yaml(fga.grpc_endpoint(), &store, "")).await;

    // Same store found by name, identical model NOT re-applied (append-only
    // hygiene): the pinned version is the same.
    assert_eq!(h1.store_id(), h2.store_id());
    assert_eq!(h1.model_id(), h2.model_id());
}

#[r2e_core::test]
#[ignore = "requires Docker"]
async fn changed_model_appends_new_version() {
    let fga = DevOpenFga::shared().await;
    let store = unique_store("append");
    let (h1, _, _) = boot(yaml(fga.grpc_endpoint(), &store, "")).await;

    let app = AppBuilder::new()
        .override_config(yaml(fga.grpc_endpoint(), &store, ""))
        .load_config::<()>()
        .plugin(OpenFga::model(authz_v2::MODEL))
        .build_state()
        .await;
    let h2 = app.state().get::<OpenFgaHandle>();

    assert_eq!(h1.store_id(), h2.store_id());
    assert_ne!(h1.model_id(), h2.model_id(), "changed model appends a new version");
}

#[r2e_core::test]
#[ignore = "requires Docker"]
async fn verify_mode_accepts_matching_model_and_pins_it() {
    let fga = DevOpenFga::shared().await;
    let store = unique_store("verify-ok");
    let (h1, _, _) = boot(yaml(fga.grpc_endpoint(), &store, "")).await;

    let (h2, _, _) = boot(yaml(fga.grpc_endpoint(), &store, "  apply_model: false\n")).await;
    assert_eq!(h1.model_id(), h2.model_id());
}

#[r2e_core::test]
#[ignore = "requires Docker"]
#[should_panic(expected = "does not match")]
async fn verify_mode_rejects_mismatched_model() {
    let fga = DevOpenFga::shared().await;
    let store = unique_store("verify-bad");
    // Seed the store with the v1 model…
    let (_h, _, _) = boot(yaml(fga.grpc_endpoint(), &store, "")).await;

    // …then verify-boot with a different compiled-in model: startup must fail.
    let _ = AppBuilder::new()
        .override_config(yaml(fga.grpc_endpoint(), &store, "  apply_model: false\n"))
        .load_config::<()>()
        .plugin(OpenFga::model(authz_v2::MODEL))
        .build_state()
        .await;
}

#[r2e_core::test]
#[ignore = "requires Docker"]
#[should_panic(expected = "never creates stores")]
async fn verify_mode_rejects_missing_store() {
    let fga = DevOpenFga::shared().await;
    let store = unique_store("verify-nostore");
    let _ = boot(yaml(fga.grpc_endpoint(), &store, "  apply_model: false\n")).await;
}

/// End-to-end through the provided beans: typed grant via `FgaClient`, cached
/// check via `OpenFgaRegistry` — all against the plugin-managed store, with
/// the pinned model id on every request.
#[r2e_core::test]
#[ignore = "requires Docker"]
async fn typed_grant_and_check_through_plugin_beans() {
    let fga = DevOpenFga::shared().await;
    let store = unique_store("grant");
    let (_, registry, client) = boot(yaml(fga.grpc_endpoint(), &store, "")).await;

    let alice = authz::user::id("alice");
    let doc = authz::document::id("readme");

    assert!(!client.check(&alice, authz::document::viewer, &doc).await.unwrap());
    client
        .grant(&alice, authz::document::viewer, &doc)
        .await
        .unwrap();
    assert!(client.check(&alice, authz::document::viewer, &doc).await.unwrap());
    assert!(registry
        .check("user:alice", "viewer", "document:readme")
        .await
        .unwrap());
}
