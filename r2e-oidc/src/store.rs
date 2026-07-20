use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

/// Result type returned by user stores.
pub type StoreResult<T> = Result<T, UserStoreError>;

/// User-store failure.
#[derive(Debug, Clone)]
pub struct UserStoreError {
    message: String,
}

impl UserStoreError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for UserStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for UserStoreError {}

/// An OIDC user profile.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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

/// Pluggable user store for the local issuer.
///
/// Implement this trait to back the local issuer with your own storage
/// (SQLx, Redis, LDAP, etc.).
pub trait UserStore: Send + Sync + 'static {
    /// Find a user by username (used during password grant).
    fn find_by_username(
        &self,
        username: &str,
    ) -> impl Future<Output = StoreResult<Option<OidcUser>>> + Send;
    /// Verify a user's password.
    fn verify_password(
        &self,
        username: &str,
        password: &str,
    ) -> impl Future<Output = StoreResult<bool>> + Send;
    /// Find a user by subject identifier (used by userinfo endpoint).
    fn find_by_sub(&self, sub: &str) -> impl Future<Output = StoreResult<Option<OidcUser>>> + Send;

    /// Authenticate and return the matched user in one store operation.
    fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> impl Future<Output = StoreResult<Option<OidcUser>>> + Send;
}

/// Object-safe wrapper for `UserStore`.
pub(crate) trait UserStoreErased: Send + Sync {
    fn find_by_sub<'a>(
        &'a self,
        sub: &'a str,
    ) -> Pin<Box<dyn Future<Output = StoreResult<Option<OidcUser>>> + Send + 'a>>;
    fn authenticate<'a>(
        &'a self,
        username: &'a str,
        password: &'a str,
    ) -> Pin<Box<dyn Future<Output = StoreResult<Option<OidcUser>>> + Send + 'a>>;
}

impl<T: UserStore> UserStoreErased for T {
    fn find_by_sub<'a>(
        &'a self,
        sub: &'a str,
    ) -> Pin<Box<dyn Future<Output = StoreResult<Option<OidcUser>>> + Send + 'a>> {
        Box::pin(UserStore::find_by_sub(self, sub))
    }

    fn authenticate<'a>(
        &'a self,
        username: &'a str,
        password: &'a str,
    ) -> Pin<Box<dyn Future<Output = StoreResult<Option<OidcUser>>> + Send + 'a>> {
        Box::pin(UserStore::authenticate(self, username, password))
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
    /// Dummy hash verified for unknown users to reduce timing enumeration.
    dummy_hash: Arc<String>,
}

impl InMemoryUserStore {
    /// Create a new empty in-memory user store.
    pub fn new() -> Self {
        Self {
            users: Arc::new(DashMap::new()),
            sub_index: Arc::new(DashMap::new()),
            dummy_hash: Arc::new(
                hash_secret("r2e-oidc-dummy-password").expect("failed to hash dummy password"),
            ),
        }
    }

    /// Add a user with a plaintext password (hashed with argon2).
    pub fn add_user(self, username: impl Into<String>, password: &str, user: OidcUser) -> Self {
        self.try_add_user(username, password, user)
            .expect("invalid in-memory OIDC user")
    }

    /// Add a user, returning validation/hash errors instead of panicking.
    pub fn try_add_user(
        self,
        username: impl Into<String>,
        password: &str,
        user: OidcUser,
    ) -> StoreResult<Self> {
        let username = username.into();
        validate_user_input(&username, password, &user)?;
        let password_hash = hash_secret(password)?;

        if let Some(existing_username) = self.sub_index.get(&user.sub) {
            if existing_username.value() != &username {
                return Err(UserStoreError::new(format!(
                    "subject '{}' is already assigned to another user",
                    user.sub
                )));
            }
        }

        if let Some(old_entry) = self.users.get(&username) {
            let old_sub = old_entry.value().0.sub.clone();
            if old_sub != user.sub {
                self.sub_index.remove(&old_sub);
            }
        }

        self.sub_index.insert(user.sub.clone(), username.clone());
        self.users.insert(username, (user, password_hash));
        Ok(self)
    }
}

impl Default for InMemoryUserStore {
    fn default() -> Self {
        Self::new()
    }
}

impl UserStore for InMemoryUserStore {
    fn find_by_username(
        &self,
        username: &str,
    ) -> impl Future<Output = StoreResult<Option<OidcUser>>> + Send {
        let result = self
            .users
            .get(username)
            .map(|entry| entry.value().0.clone());
        async move { Ok(result) }
    }

    fn verify_password(
        &self,
        username: &str,
        password: &str,
    ) -> impl Future<Output = StoreResult<bool>> + Send {
        let (hash_str, user_exists) = self
            .users
            .get(username)
            .map(|e| (e.value().1.clone(), true))
            .unwrap_or_else(|| ((*self.dummy_hash).clone(), false));
        let password = password.to_string();
        async move {
            // Run argon2 verification in a blocking task to avoid blocking the async runtime.
            let matches = tokio::task::spawn_blocking(move || verify_secret(&hash_str, &password))
                .await
                .map_err(|e| {
                    UserStoreError::new(format!("password verification task failed: {e}"))
                })??;
            Ok(user_exists && matches)
        }
    }

    fn find_by_sub(&self, sub: &str) -> impl Future<Output = StoreResult<Option<OidcUser>>> + Send {
        let result = self.sub_index.get(sub).and_then(|username_ref| {
            self.users
                .get(username_ref.value())
                .map(|entry| entry.value().0.clone())
        });
        async move { Ok(result) }
    }

    fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> impl Future<Output = StoreResult<Option<OidcUser>>> + Send {
        let entry = self
            .users
            .get(username)
            .map(|e| (e.value().0.clone(), e.value().1.clone()));
        let dummy_hash = (*self.dummy_hash).clone();
        let password = password.to_string();
        async move {
            let (user, hash_str) = match entry {
                Some((user, hash)) => (Some(user), hash),
                None => (None, dummy_hash),
            };

            let matches = tokio::task::spawn_blocking(move || verify_secret(&hash_str, &password))
                .await
                .map_err(|e| {
                    UserStoreError::new(format!("password verification task failed: {e}"))
                })??;

            Ok(user.filter(|_| matches))
        }
    }
}

fn validate_user_input(username: &str, password: &str, user: &OidcUser) -> StoreResult<()> {
    if username.trim().is_empty() {
        return Err(UserStoreError::new("username must not be empty"));
    }
    if password.is_empty() {
        return Err(UserStoreError::new("password must not be empty"));
    }
    if user.sub.trim().is_empty() {
        return Err(UserStoreError::new("user subject must not be empty"));
    }
    Ok(())
}

fn hash_secret(secret: &str) -> StoreResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(secret.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| UserStoreError::new(format!("failed to hash secret: {e}")))
}

fn verify_secret(hash_str: &str, secret: &str) -> StoreResult<bool> {
    let parsed = PasswordHash::new(hash_str)
        .map_err(|e| UserStoreError::new(format!("invalid stored password hash: {e}")))?;
    Ok(Argon2::default()
        .verify_password(secret.as_bytes(), &parsed)
        .is_ok())
}
