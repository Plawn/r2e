use std::time::Duration;

use testcontainers::core::wait::HttpWaitStrategy;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::{GenericImage, ImageExt};

use crate::service::{DevService, DevServiceSpec};

/// Default image tag. Pinned so the shared-container fingerprint stays stable
/// across runs (an unpinned `latest` would silently change the identity).
const DEFAULT_TAG: &str = "v1.8.4";
/// OpenFGA's HTTP API port (store/model/tuple bootstrap + `/healthz`).
const HTTP_PORT: u16 = 8080;
/// OpenFGA's gRPC port — the endpoint `r2e-openfga`'s `GrpcBackend` connects to.
const GRPC_PORT: u16 = 8081;

/// A containerized [OpenFGA](https://openfga.dev/) server for tests.
///
/// Runs `openfga/openfga run` with the default in-memory datastore. The gRPC
/// endpoint ([`grpc_endpoint`](Self::grpc_endpoint)) is what the `r2e-openfga`
/// `GrpcBackend` connects to (`openfga.endpoint` config); the HTTP endpoint
/// ([`http_endpoint`](Self::http_endpoint)) backs the store/model/tuple
/// bootstrap helpers.
///
/// With the `r2e-openfga` `OpenFga` plugin, a test only injects
/// `openfga.endpoint` (plus a unique `openfga.store` name for isolation on the
/// shared container) — the plugin creates the store and applies the model at
/// boot, and tuples are seeded through the typed `FgaClient` bean.
///
/// For non-plugin wiring (explicit `store_id`/`model_id` config), the HTTP
/// bootstrap helpers remain: [`create_store`], [`write_model`], and
/// [`write_tuples`] drive OpenFGA's HTTP API so a test can set up
/// authorization data and inject the resulting IDs via
/// `override_config_value` in a few lines.
///
/// [`create_store`]: Self::create_store
/// [`write_model`]: Self::write_model
/// [`write_tuples`]: Self::write_tuples
pub struct DevOpenFga {
    /// The isolated container this handle owns. `None` on the shared path:
    /// there the container belongs to the process-wide registry and outlives
    /// every handle, so the handle is a cheap copy of the endpoints.
    _container: Option<DevService>,
    http_url: String,
    grpc_url: String,
    client: reqwest::Client,
}

/// Identity and request of the OpenFGA container.
///
/// The configuration string is kept byte-identical to the pre-`DevService`
/// one so existing shared containers stay valid. A different OpenFGA image
/// is out of scope here — build a [`DevService`] directly for that.
fn spec() -> DevServiceSpec<GenericImage> {
    DevServiceSpec::new("openfga", || base_image().with_cmd(["run"]))
        .with_port(HTTP_PORT)
        .with_port(GRPC_PORT)
        .with_configuration(format!(
            "image=openfga/openfga:{DEFAULT_TAG};http={HTTP_PORT};grpc={GRPC_PORT}"
        ))
}

/// Base image: `openfga/openfga` running `run`, both ports exposed, ready once
/// `/healthz` returns 200.
fn base_image() -> GenericImage {
    GenericImage::new("openfga/openfga", DEFAULT_TAG)
        .with_exposed_port(HTTP_PORT.tcp())
        .with_exposed_port(GRPC_PORT.tcp())
        .with_wait_for(WaitFor::http(
            HttpWaitStrategy::new("/healthz")
                .with_port(HTTP_PORT.tcp())
                .with_expected_status_code(200u16),
        ))
}

impl DevOpenFga {
    /// Start a fresh, isolated OpenFGA container (`openfga/openfga:v1.8.4`).
    ///
    /// # Panics
    ///
    /// Panics if Docker is unavailable or the container fails to start.
    pub async fn start() -> Self {
        let service = DevService::start(spec()).await;
        let mut handle = Self::describe(&service);
        handle._container = Some(service);
        handle
    }

    /// The cross-process shared OpenFGA container, started on first use.
    ///
    /// Tests sharing the container must not assume an empty server — create a
    /// dedicated store per test with [`create_store`](Self::create_store), or
    /// use [`start`](Self::start) for a private container.
    ///
    /// The *container* is shared; the returned handle is a cheap owned copy of
    /// its endpoints, so dropping it stops nothing.
    pub async fn shared() -> Self {
        Self::describe(DevService::shared(spec()).await)
    }

    /// The endpoints of a running container, without owning it.
    fn describe(service: &DevService) -> Self {
        let host = service.host();
        Self {
            _container: None,
            http_url: format!("http://{host}:{}", service.port(HTTP_PORT)),
            grpc_url: format!("http://{host}:{}", service.port(GRPC_PORT)),
            client: reqwest::Client::new(),
        }
    }

    /// gRPC endpoint (`http://{host}:{port}`), for `openfga.endpoint` config.
    pub fn grpc_endpoint(&self) -> &str {
        &self.grpc_url
    }

    /// HTTP API endpoint (`http://{host}:{port}`), for the bootstrap helpers.
    pub fn http_endpoint(&self) -> &str {
        &self.http_url
    }

    /// Create a store via the HTTP API and return its generated ID.
    ///
    /// # Panics
    ///
    /// Panics if the request fails or OpenFGA returns a non-success status.
    pub async fn create_store(&self, name: &str) -> String {
        let body = serde_json::json!({ "name": name });
        let resp: serde_json::Value = self.post("/stores", &body).await;
        resp["id"]
            .as_str()
            .expect("OpenFGA create-store response missing `id`")
            .to_string()
    }

    /// Write an authorization model (schema 1.1 JSON) to a store and return the
    /// generated model ID.
    ///
    /// The `model` is the OpenFGA JSON model, e.g.:
    ///
    /// ```ignore
    /// serde_json::json!({
    ///     "schema_version": "1.1",
    ///     "type_definitions": [
    ///         { "type": "user" },
    ///         {
    ///             "type": "document",
    ///             "relations": { "viewer": { "this": {} } },
    ///             "metadata": { "relations": {
    ///                 "viewer": { "directly_related_user_types": [{ "type": "user" }] }
    ///             } }
    ///         }
    ///     ]
    /// })
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the request fails or OpenFGA returns a non-success status.
    pub async fn write_model(&self, store_id: &str, model: &serde_json::Value) -> String {
        let resp: serde_json::Value = self
            .post(&format!("/stores/{store_id}/authorization-models"), model)
            .await;
        resp["authorization_model_id"]
            .as_str()
            .expect("OpenFGA write-model response missing `authorization_model_id`")
            .to_string()
    }

    /// Write relationship tuples to a store as `(user, relation, object)`
    /// triples (each fully qualified, e.g. `("user:alice", "viewer", "document:1")`).
    ///
    /// # Panics
    ///
    /// Panics if the request fails or OpenFGA returns a non-success status
    /// (writing a duplicate tuple is an error in OpenFGA).
    pub async fn write_tuples(
        &self,
        store_id: &str,
        model_id: &str,
        tuples: &[(&str, &str, &str)],
    ) {
        if tuples.is_empty() {
            return;
        }
        let tuple_keys: Vec<serde_json::Value> = tuples
            .iter()
            .map(|(user, relation, object)| {
                serde_json::json!({ "user": user, "relation": relation, "object": object })
            })
            .collect();
        let body = serde_json::json!({
            "authorization_model_id": model_id,
            "writes": { "tuple_keys": tuple_keys },
        });
        let _: serde_json::Value = self.post(&format!("/stores/{store_id}/write"), &body).await;
    }

    async fn post(&self, path: &str, body: &serde_json::Value) -> serde_json::Value {
        let url = format!("{}{path}", self.http_url);
        let response = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(10))
            .json(body)
            .send()
            .await
            .unwrap_or_else(|e| panic!("OpenFGA request to {url} failed: {e}"));
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            panic!("OpenFGA request to {url} returned {status}: {text}");
        }
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    }
}
