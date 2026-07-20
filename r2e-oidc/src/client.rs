use std::collections::HashMap;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};

use crate::store::UserStoreError;

/// Registry of OAuth 2.0 clients for the `client_credentials` grant.
pub struct ClientRegistry {
    /// Map: client_id -> hashed_secret
    clients: HashMap<String, String>,
    /// Dummy hash verified for unknown clients to reduce timing enumeration.
    dummy_hash: String,
}

impl ClientRegistry {
    /// Create an empty client registry.
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            dummy_hash: hash_secret("r2e-oidc-dummy-client-secret")
                .expect("failed to hash dummy client secret"),
        }
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
        tokio::task::spawn_blocking(move || verify_secret(&hash_str, &client_secret))
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
