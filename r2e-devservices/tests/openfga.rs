//! OpenFGA dev-service smoke test.
//!
//! Requires a running Docker daemon and is `#[ignore]`d by default:
//!
//! ```bash
//! cargo test -p r2e-devservices --features openfga --test openfga -- --ignored
//! ```
#![cfg(feature = "openfga")]

use r2e_devservices::DevOpenFga;

#[tokio::test]
#[ignore = "requires Docker"]
async fn openfga_dev_service_boots_and_bootstraps() {
    let fga = DevOpenFga::shared().await;
    assert!(fga.grpc_endpoint().starts_with("http://"));
    assert!(fga.http_endpoint().starts_with("http://"));

    // shared() returns the same container on subsequent calls.
    let again = DevOpenFga::shared().await;
    assert_eq!(fga.grpc_endpoint(), again.grpc_endpoint());

    // Full HTTP bootstrap path: create store → write model → write tuples.
    let store_id = fga.create_store("smoke-test").await;
    assert!(!store_id.is_empty());

    let model = serde_json::json!({
        "schema_version": "1.1",
        "type_definitions": [
            { "type": "user" },
            {
                "type": "document",
                "relations": { "viewer": { "this": {} } },
                "metadata": {
                    "relations": {
                        "viewer": { "directly_related_user_types": [{ "type": "user" }] }
                    }
                }
            }
        ]
    });
    let model_id = fga.write_model(&store_id, &model).await;
    assert!(!model_id.is_empty());

    fga.write_tuples(&store_id, &model_id, &[("user:alice", "viewer", "document:readme")])
        .await;
}
