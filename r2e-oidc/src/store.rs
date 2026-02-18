use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

/// An OIDC user profile.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OidcUser {
    /// Unique subject identifier.
    pub sub: String,
    /// Email address.
    pub email: Option<String>,
    /// Roles for authorization.
    pub roles: Vec<String>,
    /// Additional claims to include in the JWT.
    pub extra_claims: HashMap<String, serde_json::Value>,
}

impl Default for OidcUser {
    fn default() -> Self {
        Self {
            sub: String::new(),
            email: None,
            roles: Vec::new(),
            extra_claims: HashMap::new(),
        }
    }
}

/// Pluggable user store for the OIDC server.
///
/// Implement this trait to back the OIDC server with your own storage
/// (SQLx, Redis, LDAP, etc.).
pub trait UserStore: Send + Sync + 'static {
    /// Find a user by username (used during password grant).
    fn find_by_username(&self, username: &str) -> impl Future<Output = Option<OidcUser>> + Send;
    /// Verify a user's password.
    fn verify_password(&self, username: &str, password: &str) -> impl Future<Output = bool> + Send;
    /// Find a user by subject identifier (used by userinfo endpoint).
    fn find_by_sub(&self, sub: &str) -> impl Future<Output = Option<OidcUser>> + Send;
}

/// Object-safe wrapper for `UserStore`.
pub(crate) trait UserStoreErased: Send + Sync {
    fn find_by_username<'a>(
        &'a self,
        username: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<OidcUser>> + Send + 'a>>;
    fn verify_password<'a>(
        &'a self,
        username: &'a str,
        password: &'a str,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>>;
    fn find_by_sub<'a>(
        &'a self,
        sub: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<OidcUser>> + Send + 'a>>;
}

impl<T: UserStore> UserStoreErased for T {
    fn find_by_username<'a>(
        &'a self,
        username: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<OidcUser>> + Send + 'a>> {
        Box::pin(UserStore::find_by_username(self, username))
    }

    fn verify_password<'a>(
        &'a self,
        username: &'a str,
        password: &'a str,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(UserStore::verify_password(self, username, password))
    }

    fn find_by_sub<'a>(
        &'a self,
        sub: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<OidcUser>> + Send + 'a>> {
        Box::pin(UserStore::find_by_sub(self, sub))
    }
}

/// In-memory user store for development and testing.
///
/// Passwords are hashed with argon2.
pub struct InMemoryUserStore {
    /// Map: username -> (OidcUser, password_hash)
    users: Arc<DashMap<String, (OidcUser, String)>>,
    /// Index: sub -> username (for find_by_sub)
    sub_index: Arc<DashMap<String, String>>,
}

impl InMemoryUserStore {
    /// Create a new empty in-memory user store.
    pub fn new() -> Self {
        Self {
            users: Arc::new(DashMap::new()),
            sub_index: Arc::new(DashMap::new()),
        }
    }

    /// Add a user with a plaintext password (hashed with argon2).
    pub fn add_user(self, username: impl Into<String>, password: &str, user: OidcUser) -> Self {
        let username = username.into();
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .expect("failed to hash password")
            .to_string();

        self.sub_index
            .insert(user.sub.clone(), username.clone());
        self.users.insert(username, (user, password_hash));
        self
    }
}

impl Default for InMemoryUserStore {
    fn default() -> Self {
        Self::new()
    }
}

impl UserStore for InMemoryUserStore {
    fn find_by_username(&self, username: &str) -> impl Future<Output = Option<OidcUser>> + Send {
        let result = self
            .users
            .get(username)
            .map(|entry| entry.value().0.clone());
        async move { result }
    }

    fn verify_password(
        &self,
        username: &str,
        password: &str,
    ) -> impl Future<Output = bool> + Send {
        let entry = self.users.get(username).map(|e| e.value().1.clone());
        let password = password.to_string();
        async move {
            let Some(hash_str) = entry else {
                return false;
            };
            // Run argon2 verification in a blocking task to avoid blocking the async runtime.
            tokio::task::spawn_blocking(move || {
                let parsed = PasswordHash::new(&hash_str).expect("invalid stored password hash");
                Argon2::default()
                    .verify_password(password.as_bytes(), &parsed)
                    .is_ok()
            })
            .await
            .unwrap_or(false)
        }
    }

    fn find_by_sub(&self, sub: &str) -> impl Future<Output = Option<OidcUser>> + Send {
        let result = self.sub_index.get(sub).and_then(|username_ref| {
            self.users
                .get(username_ref.value())
                .map(|entry| entry.value().0.clone())
        });
        async move { result }
    }
}
