use std::collections::{HashMap, HashSet};

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};

use crate::store::UserStoreError;

/// Registry of confidential clients and public Authorization Code + PKCE
/// clients.
pub struct ClientRegistry {
    /// Map: client_id -> hashed_secret
    clients: HashMap<String, String>,
    /// Public client id -> exact redirect URI allowlist.
    public_clients: HashMap<String, HashSet<String>>,
    /// Dummy hash verified for unknown clients to reduce timing enumeration.
    dummy_hash: String,
}

impl ClientRegistry {
    /// Create an empty client registry.
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            public_clients: HashMap::new(),
            dummy_hash: hash_secret("r2e-oidc-dummy-client-secret")
                .expect("failed to hash dummy client secret"),
        }
    }

    /// Register a public client. Every redirect URI is matched exactly during
    /// authorization; wildcards are deliberately unsupported.
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
            if url.scheme() == "http"
                && !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
            {
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
        self.public_clients.insert(client_id, redirects);
        Ok(self)
    }

    /// Register a client. The secret is hashed with argon2.
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
        let hash = hash_secret(client_secret)?;
        self.clients.insert(client_id, hash);
        Ok(self)
    }

    pub(crate) fn hash_for_validation(&self, client_id: &str) -> (String, bool) {
        self.clients
            .get(client_id)
            .cloned()
            .map(|hash| (hash, true))
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
            .is_some_and(|uris| uris.contains(redirect_uri))
    }
}

impl Default for ClientRegistry {
    fn default() -> Self {
        Self::new()
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
