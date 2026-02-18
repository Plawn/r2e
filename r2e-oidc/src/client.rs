use std::collections::HashMap;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};

/// Registry of OAuth 2.0 clients for the `client_credentials` grant.
pub struct ClientRegistry {
    /// Map: client_id -> hashed_secret
    clients: HashMap<String, String>,
}

impl ClientRegistry {
    /// Create an empty client registry.
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
        }
    }

    /// Register a client. The secret is hashed with argon2.
    pub fn add_client(mut self, client_id: impl Into<String>, client_secret: &str) -> Self {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let hash = argon2
            .hash_password(client_secret.as_bytes(), &salt)
            .expect("failed to hash client secret")
            .to_string();
        self.clients.insert(client_id.into(), hash);
        self
    }

    /// Validate client credentials asynchronously.
    ///
    /// Returns `true` if the client exists and the secret matches.
    /// Uses `spawn_blocking` to avoid blocking the async runtime during argon2 verification.
    pub(crate) async fn validate(&self, client_id: &str, client_secret: &str) -> bool {
        let Some(hash_str) = self.clients.get(client_id).cloned() else {
            return false;
        };
        let secret = client_secret.to_string();
        tokio::task::spawn_blocking(move || {
            let parsed = match PasswordHash::new(&hash_str) {
                Ok(h) => h,
                Err(_) => return false,
            };
            Argon2::default()
                .verify_password(secret.as_bytes(), &parsed)
                .is_ok()
        })
        .await
        .unwrap_or(false)
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
