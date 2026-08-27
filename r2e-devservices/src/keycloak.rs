use std::time::Duration;

use testcontainers::core::wait::HttpWaitStrategy;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::{GenericImage, ImageExt};

use crate::service::{DevService, DevServiceSpec};

/// Default image tag. Pinned so the shared-container fingerprint stays stable
/// across runs (an unpinned `latest` would silently change the identity).
const DEFAULT_TAG: &str = "26.4";
/// Keycloak's HTTP port in `start-dev` mode.
const HTTP_PORT: u16 = 8080;
/// Bootstrap admin credentials (dev container only).
const ADMIN_USER: &str = "admin";
const ADMIN_PASSWORD: &str = "admin";

/// The realm imported by default, bundled with the crate.
///
/// Realm `r2e-mcp` ships everything an MCP OAuth test needs:
///
/// - public client `mcp-public` — authorization code + PKCE (S256), redirect
///   URIs for localhost and claude.ai/claude.com callbacks; default scope
///   `mcp` carries an **audience mapper** stamping
///   `http://localhost:3000/mcp` into the token `aud`, optional scopes
///   `mcp:read` / `mcp:write`
/// - public client `test-cli` — direct access grants, for
///   [`password_token`](DevKeycloak::password_token)
/// - confidential client `mcp-introspect` (secret `introspect-secret`) — for
///   RFC 7662 introspection
/// - users `alice` / `alice-password` (roles `admin`, `user`) and
///   `bob` / `bob-password` (role `user`)
pub const DEFAULT_REALM_JSON: &str = include_str!("../resources/keycloak-realm.json");

/// A containerized [Keycloak](https://www.keycloak.org/) server for tests.
///
/// Runs `quay.io/keycloak/keycloak start-dev --import-realm` with a realm
/// imported from JSON — [`DEFAULT_REALM_JSON`] unless
/// [`start_with`](Self::start_with) / [`shared_with`](Self::shared_with) pass
/// another. The realm file is copied into the container before start and is
/// part of the shared-container identity, so two different realms get two
/// containers.
///
/// Wire it to an MCP resource server by pointing the issuer at
/// [`issuer`](Self::issuer) and pinning the resource to the audience the
/// bundled realm stamps:
///
/// ```ignore
/// let kc = DevKeycloak::shared().await;
/// let app = builder
///     .override_config_value("mcp.auth.issuer", kc.issuer())
///     .override_config_value("mcp.auth.allow-insecure", true)
///     .override_config_value("mcp.auth.resource", "http://localhost:3000/mcp")
///     ...;
/// let token = kc.password_token("alice", "alice-password", "test-cli", "mcp:read").await;
/// ```
pub struct DevKeycloak {
    /// The isolated container this handle owns. `None` on the shared path:
    /// there the container belongs to the process-wide registry and outlives
    /// every handle, so the handle is a cheap copy of the endpoints.
    _container: Option<DevService>,
    base_url: String,
    realm: String,
    client: reqwest::Client,
}

/// Identity and request of the Keycloak container.
///
/// The realm JSON is cloned into the request factory: the factory must build
/// the same request every time, and the copied bytes are digested into the
/// shared-container identity.
fn spec(realm_json: &str) -> DevServiceSpec<GenericImage> {
    let realm_json = realm_json.as_bytes().to_vec();
    let realm = realm_name(&realm_json);
    DevServiceSpec::new("keycloak", move || {
        base_image(&realm)
            .with_cmd(["start-dev", "--import-realm"])
            // KC_BOOTSTRAP_ADMIN_* is the Keycloak 26 name; KEYCLOAK_ADMIN
            // keeps older tags working with the same spec.
            .with_env_var("KC_BOOTSTRAP_ADMIN_USERNAME", ADMIN_USER)
            .with_env_var("KC_BOOTSTRAP_ADMIN_PASSWORD", ADMIN_PASSWORD)
            .with_env_var("KEYCLOAK_ADMIN", ADMIN_USER)
            .with_env_var("KEYCLOAK_ADMIN_PASSWORD", ADMIN_PASSWORD)
            .with_copy_to("/opt/keycloak/data/import/realm.json", realm_json.clone())
            // A cold `start-dev` + realm import routinely exceeds the 60 s
            // testcontainers default.
            .with_startup_timeout(Duration::from_secs(180))
    })
    .with_port(HTTP_PORT)
}

/// Base image: ready once the imported realm's discovery document answers 200
/// (which also proves the import worked — a missing realm 404s).
fn base_image(realm: &str) -> GenericImage {
    GenericImage::new("quay.io/keycloak/keycloak", DEFAULT_TAG)
        .with_exposed_port(HTTP_PORT.tcp())
        .with_wait_for(WaitFor::http(
            HttpWaitStrategy::new(format!(
                "/realms/{realm}/.well-known/openid-configuration"
            ))
            .with_port(HTTP_PORT.tcp())
            .with_expected_status_code(200u16),
        ))
}

/// The `realm` field of the import JSON — the name the container serves it
/// under, needed for the readiness URL and [`DevKeycloak::issuer`].
fn realm_name(realm_json: &[u8]) -> String {
    let doc: serde_json::Value = serde_json::from_slice(realm_json)
        .expect("DevKeycloak realm import is not valid JSON");
    doc["realm"]
        .as_str()
        .expect("DevKeycloak realm import has no top-level `realm` name")
        .to_string()
}

impl DevKeycloak {
    /// Start a fresh, isolated Keycloak container importing
    /// [`DEFAULT_REALM_JSON`].
    ///
    /// # Panics
    ///
    /// Panics if Docker is unavailable or the container fails to start.
    pub async fn start() -> Self {
        Self::start_with(DEFAULT_REALM_JSON).await
    }

    /// Start a fresh, isolated container importing the given realm JSON
    /// (Keycloak's realm-export format, top-level `realm` name required).
    pub async fn start_with(realm_json: &str) -> Self {
        let realm = realm_name(realm_json.as_bytes());
        let service = DevService::start(spec(realm_json)).await;
        let mut handle = Self::describe(&service, realm);
        handle._container = Some(service);
        handle
    }

    /// The cross-process shared Keycloak container importing
    /// [`DEFAULT_REALM_JSON`], started on first use.
    ///
    /// The *container* is shared; the returned handle is a cheap owned copy of
    /// its endpoints, so dropping it stops nothing. Tests sharing it must
    /// treat the realm as read-only fixture data.
    pub async fn shared() -> Self {
        Self::shared_with(DEFAULT_REALM_JSON).await
    }

    /// The shared container for a *custom* realm JSON. Each distinct realm
    /// gets a container of its own (the copied bytes are part of the
    /// identity).
    pub async fn shared_with(realm_json: &str) -> Self {
        let realm = realm_name(realm_json.as_bytes());
        Self::describe(DevService::shared(spec(realm_json)).await, realm)
    }

    /// The endpoints of a running container, without owning it.
    fn describe(service: &DevService, realm: String) -> Self {
        Self {
            _container: None,
            base_url: format!("http://{}", service.endpoint(HTTP_PORT)),
            realm,
            client: reqwest::Client::new(),
        }
    }

    /// Base URL (`http://{host}:{port}`), no trailing slash.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The imported realm's name.
    pub fn realm(&self) -> &str {
        &self.realm
    }

    /// The realm's issuer URL — what `mcp.auth.issuer` (or
    /// `security.jwt.issuer`) should be set to.
    pub fn issuer(&self) -> String {
        format!("{}/realms/{}", self.base_url, self.realm)
    }

    /// Mint a user access token via the direct-access (password) grant.
    ///
    /// With [`DEFAULT_REALM_JSON`], `client_id` is `"test-cli"`; pass the
    /// optional scopes to request in `scope` (space-separated, e.g.
    /// `"mcp:read mcp:write"`, or `""` for defaults only).
    ///
    /// # Panics
    ///
    /// Panics if the token request fails — wrong credentials, unknown client,
    /// or a scope the client does not have.
    pub async fn password_token(
        &self,
        username: &str,
        password: &str,
        client_id: &str,
        scope: &str,
    ) -> String {
        let mut form = vec![
            ("grant_type", "password"),
            ("client_id", client_id),
            ("username", username),
            ("password", password),
        ];
        if !scope.is_empty() {
            form.push(("scope", scope));
        }
        self.token(&self.realm, &form).await
    }

    /// Mint an access token for a confidential client via the
    /// client-credentials grant (with [`DEFAULT_REALM_JSON`]:
    /// `"mcp-introspect"` / `"introspect-secret"`).
    pub async fn client_token(&self, client_id: &str, client_secret: &str) -> String {
        self.token(
            &self.realm,
            &[
                ("grant_type", "client_credentials"),
                ("client_id", client_id),
                ("client_secret", client_secret),
            ],
        )
        .await
    }

    /// Mint a `master`-realm admin token (bootstrap `admin`/`admin`), for
    /// driving Keycloak's admin REST API directly in a test.
    pub async fn admin_token(&self) -> String {
        self.token(
            "master",
            &[
                ("grant_type", "password"),
                ("client_id", "admin-cli"),
                ("username", ADMIN_USER),
                ("password", ADMIN_PASSWORD),
            ],
        )
        .await
    }

    async fn token(&self, realm: &str, form: &[(&str, &str)]) -> String {
        let url = format!("{}/realms/{realm}/protocol/openid-connect/token", self.base_url);
        let response = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(10))
            .form(form)
            .send()
            .await
            .unwrap_or_else(|e| panic!("Keycloak token request to {url} failed: {e}"));
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            panic!("Keycloak token request to {url} returned {status}: {text}");
        }
        let doc: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("Keycloak token response is not JSON: {e}"));
        doc["access_token"]
            .as_str()
            .expect("Keycloak token response missing `access_token`")
            .to_string()
    }
}
