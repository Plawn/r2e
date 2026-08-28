//! One-time Authorization Code + PKCE state.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use dashmap::DashMap;
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};

use crate::store::OidcUser;

pub(crate) struct AuthorizationGrant {
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub scope: String,
    pub resource: Option<String>,
    pub user: OidcUser,
    expires_at: Instant,
}

#[derive(Clone)]
pub(crate) struct AuthorizationCodeStore {
    codes: Arc<DashMap<String, AuthorizationGrant>>,
    ttl: Duration,
}

impl AuthorizationCodeStore {
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            codes: Arc::new(DashMap::new()),
            ttl,
        }
    }

    pub(crate) fn issue(&self, mut grant: AuthorizationGrant) -> String {
        self.codes
            .retain(|_, grant| grant.expires_at > Instant::now());
        grant.expires_at = Instant::now() + self.ttl;
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let code = URL_SAFE_NO_PAD.encode(bytes);
        self.codes.insert(code.clone(), grant);
        code
    }

    /// Remove before validation: an authorization code is one-use even when
    /// the redemption attempt supplies a wrong verifier.
    pub(crate) fn take(&self, code: &str) -> Option<AuthorizationGrant> {
        let (_, grant) = self.codes.remove(code)?;
        (grant.expires_at > Instant::now()).then_some(grant)
    }
}

/// Lifetime of the one-time CSRF token embedded in a rendered login form.
pub(crate) const LOGIN_FORM_TTL: Duration = Duration::from_secs(600);

/// One-time CSRF tokens handed to rendered login forms.
///
/// `GET /oauth/authorize` issues one per page; `POST /oauth/authorize` must
/// echo it back, and consuming it invalidates it. Without this, any site could
/// post credentials — or a silent authorization — to the local login endpoint.
#[derive(Clone)]
pub(crate) struct LoginFormStore {
    tokens: Arc<DashMap<String, Instant>>,
    ttl: Duration,
}

impl LoginFormStore {
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            tokens: Arc::new(DashMap::new()),
            ttl,
        }
    }

    pub(crate) fn issue(&self) -> String {
        self.tokens
            .retain(|_, expires_at| *expires_at > Instant::now());
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let token = URL_SAFE_NO_PAD.encode(bytes);
        self.tokens.insert(token.clone(), Instant::now() + self.ttl);
        token
    }

    /// Consume a token: `true` only for a token issued here, still valid, and
    /// not yet used.
    pub(crate) fn consume(&self, token: &str) -> bool {
        self.tokens
            .remove(token)
            .is_some_and(|(_, expires_at)| expires_at > Instant::now())
    }
}

impl AuthorizationGrant {
    pub(crate) fn new(
        client_id: String,
        redirect_uri: String,
        code_challenge: String,
        scope: String,
        resource: Option<String>,
        user: OidcUser,
    ) -> Self {
        Self {
            client_id,
            redirect_uri,
            code_challenge,
            scope,
            resource,
            user,
            expires_at: Instant::now(),
        }
    }
}

pub(crate) fn valid_code_challenge(challenge: &str) -> bool {
    challenge.len() == 43
        && challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

pub(crate) fn valid_code_verifier(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

pub(crate) fn verify_s256(verifier: &str, expected: &str) -> bool {
    if !valid_code_verifier(verifier) {
        return false;
    }
    let actual = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    actual.len() == expected.len()
        && actual
            .bytes()
            .zip(expected.bytes())
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}
