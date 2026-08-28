use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::{Ipv4Addr, Ipv6Addr};

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use url::Host;

use crate::store::UserStoreError;
use crate::token::valid_scope_token;

/// A confidential (`client_credentials`) client.
struct ConfidentialClient {
    secret_hash: String,
    scopes: BTreeSet<String>,
}

/// A public Authorization Code + PKCE client.
struct PublicClient {
    redirect_uris: HashSet<String>,
    scopes: BTreeSet<String>,
}

/// Registry of confidential clients and public Authorization Code + PKCE
/// clients.
///
/// Every client carries a **scope allowlist**. A client may only be granted
/// the scopes it declares through [`with_scopes`](Self::with_scopes); a client
/// that declares none can only receive an empty scope (fail closed).
pub struct ClientRegistry {
    /// Map: client_id -> confidential client.
    clients: HashMap<String, ConfidentialClient>,
    /// Map: client_id -> public client (exact redirect URI allowlist).
    public_clients: HashMap<String, PublicClient>,
    /// The client most recently registered, targeted by [`Self::with_scopes`].
    last_registered: Option<String>,
    /// Dummy hash verified for unknown clients to reduce timing enumeration.
    dummy_hash: String,
}

impl ClientRegistry {
    /// Create an empty client registry.
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            public_clients: HashMap::new(),
            last_registered: None,
            dummy_hash: hash_secret("r2e-oidc-dummy-client-secret")
                .expect("failed to hash dummy client secret"),
        }
    }

    /// Register a public client. Every redirect URI is matched exactly during
    /// authorization; wildcards are deliberately unsupported.
    ///
    /// The client starts with an **empty scope allowlist** — chain
    /// [`with_scopes`](Self::with_scopes) to allow scopes.
    pub fn add_public_client(
        self,
        client_id: impl Into<String>,
        redirect_uris: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.try_add_public_client(client_id, redirect_uris)
            .expect("invalid public OIDC client")
    }

    /// Fallible form of [`add_public_client`](Self::add_public_client).
    pub fn try_add_public_client(
        mut self,
        client_id: impl Into<String>,
        redirect_uris: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, UserStoreError> {
        let client_id = client_id.into();
        if client_id.trim().is_empty() {
            return Err(UserStoreError::new("client_id must not be empty"));
        }
        if self.clients.contains_key(&client_id) {
            return Err(UserStoreError::new(
                "client_id is already registered as a confidential client",
            ));
        }
        let mut redirects = HashSet::new();
        for redirect_uri in redirect_uris {
            let redirect_uri = redirect_uri.into();
            let url = url::Url::parse(&redirect_uri)
                .map_err(|e| UserStoreError::new(format!("invalid redirect URI: {e}")))?;
            if url.fragment().is_some() || !matches!(url.scheme(), "https" | "http") {
                return Err(UserStoreError::new(
                    "redirect URI must use http(s) and must not contain a fragment",
                ));
            }
            if url.scheme() == "http" && !is_loopback_host(url.host()) {
                return Err(UserStoreError::new(
                    "http redirect URIs are only allowed for loopback clients",
                ));
            }
            redirects.insert(redirect_uri);
        }
        if redirects.is_empty() {
            return Err(UserStoreError::new(
                "a public client needs at least one redirect URI",
            ));
        }
        self.public_clients.insert(
            client_id.clone(),
            PublicClient {
                redirect_uris: redirects,
                scopes: BTreeSet::new(),
            },
        );
        self.last_registered = Some(client_id);
        Ok(self)
    }

    /// Register a client. The secret is hashed with argon2.
    ///
    /// The client starts with an **empty scope allowlist** — chain
    /// [`with_scopes`](Self::with_scopes) to allow scopes.
    pub fn add_client(self, client_id: impl Into<String>, client_secret: &str) -> Self {
        self.try_add_client(client_id, client_secret)
            .expect("invalid OIDC client")
    }

    /// Register a client, returning validation/hash errors instead of panicking.
    pub fn try_add_client(
        mut self,
        client_id: impl Into<String>,
        client_secret: &str,
    ) -> Result<Self, UserStoreError> {
        let client_id = client_id.into();
        if client_id.trim().is_empty() {
            return Err(UserStoreError::new("client_id must not be empty"));
        }
        if self.public_clients.contains_key(&client_id) {
            return Err(UserStoreError::new(
                "client_id is already registered as a public client",
            ));
        }
        if client_secret.is_empty() {
            return Err(UserStoreError::new("client_secret must not be empty"));
        }
        let secret_hash = hash_secret(client_secret)?;
        self.clients.insert(
            client_id.clone(),
            ConfidentialClient {
                secret_hash,
                scopes: BTreeSet::new(),
            },
        );
        self.last_registered = Some(client_id);
        Ok(self)
    }

    /// Set the scope allowlist of the **most recently registered** client.
    ///
    /// A requested scope outside this list is rejected with `invalid_scope`
    /// (RFC 6749 §4.1.2.1 / §5.2). A request that omits `scope` is granted the
    /// whole allowlist. Replaces (does not extend) any previous allowlist.
    ///
    /// ```ignore
    /// ClientRegistry::new()
    ///     .add_public_client("mcp-client", ["http://127.0.0.1:49152/callback"])
    ///     .with_scopes(["openid", "mcp:read"])
    ///     .add_client("worker", "worker-secret")
    ///     .with_scopes(["jobs:run"]);
    /// ```
    pub fn with_scopes(self, scopes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.try_with_scopes(scopes)
            .expect("invalid OIDC client scope allowlist")
    }

    /// Fallible form of [`with_scopes`](Self::with_scopes).
    pub fn try_with_scopes(
        mut self,
        scopes: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, UserStoreError> {
        let Some(client_id) = self.last_registered.clone() else {
            return Err(UserStoreError::new(
                "with_scopes must follow add_client or add_public_client",
            ));
        };
        let mut allowed = BTreeSet::new();
        for scope in scopes {
            let scope = scope.into();
            if !valid_scope_token(&scope) {
                return Err(UserStoreError::new(format!(
                    "invalid scope token: `{scope}`"
                )));
            }
            allowed.insert(scope);
        }
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.scopes = allowed;
        } else if let Some(client) = self.public_clients.get_mut(&client_id) {
            client.scopes = allowed;
        }
        Ok(self)
    }

    pub(crate) fn hash_for_validation(&self, client_id: &str) -> (String, bool) {
        self.clients
            .get(client_id)
            .map(|client| (client.secret_hash.clone(), true))
            .unwrap_or_else(|| (self.dummy_hash.clone(), false))
    }

    pub(crate) async fn verify_hash(
        hash_str: String,
        client_secret: String,
    ) -> Result<bool, UserStoreError> {
        r2e_core::rt::spawn_blocking(move || verify_secret(&hash_str, &client_secret))
            .await
            .map_err(|e| {
                UserStoreError::new(format!("client secret verification task failed: {e}"))
            })?
    }

    /// Validate client credentials asynchronously.
    ///
    /// Returns `true` if the client exists and the secret matches.
    /// Uses `spawn_blocking` to avoid blocking the async runtime during argon2 verification.
    pub(crate) async fn validate(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<bool, UserStoreError> {
        let (hash_str, exists) = self.hash_for_validation(client_id);
        let matches = Self::verify_hash(hash_str, client_secret.to_string()).await?;
        Ok(exists && matches)
    }

    /// Returns `true` if the registry has no clients.
    pub(crate) fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    pub(crate) fn has_public_clients(&self) -> bool {
        !self.public_clients.is_empty()
    }

    pub(crate) fn accepts_redirect(&self, client_id: &str, redirect_uri: &str) -> bool {
        self.public_clients
            .get(client_id)
            .is_some_and(|client| client.redirect_uris.contains(redirect_uri))
    }

    /// The scope allowlist of a registered client, or `None` when the client
    /// is unknown. An unknown or scope-less client can never widen a grant.
    pub(crate) fn allowed_scopes(&self, client_id: &str) -> Option<&BTreeSet<String>> {
        self.clients
            .get(client_id)
            .map(|client| &client.scopes)
            .or_else(|| {
                self.public_clients
                    .get(client_id)
                    .map(|client| &client.scopes)
            })
    }

    /// Union of every registered client's allowlist — advertised as
    /// `scopes_supported` in the discovery document.
    pub(crate) fn all_scopes(&self) -> BTreeSet<String> {
        self.clients
            .values()
            .map(|client| &client.scopes)
            .chain(self.public_clients.values().map(|client| &client.scopes))
            .flatten()
            .cloned()
            .collect()
    }
}

impl Default for ClientRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// RFC 8252 §7.3: only the loopback interface may be addressed over plain
/// HTTP. `Url::host()` (not `host_str`) is used because the string form of an
/// IPv6 host keeps its brackets (`[::1]`).
fn is_loopback_host(host: Option<Host<&str>>) -> bool {
    match host {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(addr)) => addr == Ipv4Addr::LOCALHOST,
        Some(Host::Ipv6(addr)) => addr == Ipv6Addr::LOCALHOST,
        None => false,
    }
}

fn hash_secret(secret: &str) -> Result<String, UserStoreError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(secret.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| UserStoreError::new(format!("failed to hash secret: {e}")))
}

fn verify_secret(hash_str: &str, secret: &str) -> Result<bool, UserStoreError> {
    let parsed = PasswordHash::new(hash_str)
        .map_err(|e| UserStoreError::new(format!("invalid stored client secret hash: {e}")))?;
    Ok(Argon2::default()
        .verify_password(secret.as_bytes(), &parsed)
        .is_ok())
}
