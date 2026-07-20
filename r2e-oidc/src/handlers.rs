use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine};
use r2e_core::http::extract::State;
use r2e_core::http::header;
use r2e_core::http::response::IntoResponse;
use r2e_core::http::Form;
use r2e_core::http::HeaderMap;
use r2e_core::http::Json;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::OidcError;
use crate::state::OidcState;
use crate::token::{has_scope, normalize_scope, AccessTokenClaims, DEFAULT_USER_SCOPE};

/// RFC 6749 §5.1 required headers for token responses.
type TokenResponseHeaders = [(header::HeaderName, &'static str); 2];
const TOKEN_HEADERS: TokenResponseHeaders = [
    (header::CACHE_CONTROL, "no-store"),
    (header::PRAGMA, "no-cache"),
];

/// Token request parameters (form-urlencoded).
#[derive(Debug, Deserialize)]
pub(crate) struct TokenRequest {
    pub grant_type: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub scope: Option<String>,
}

/// Token response.
#[derive(Serialize)]
pub(crate) struct TokenResponse {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: u64,
}

/// POST /oauth/token
pub(crate) async fn token_handler(
    State(state): State<Arc<OidcState>>,
    headers: HeaderMap,
    Form(req): Form<TokenRequest>,
) -> Result<impl IntoResponse, OidcError> {
    let grant_type = req
        .grant_type
        .as_deref()
        .ok_or_else(|| OidcError::InvalidRequest("missing 'grant_type' parameter".into()))?;

    let json = match grant_type {
        "password" => handle_password_grant(&state, req).await?,
        "client_credentials" => handle_client_credentials_grant(&state, &headers, &req).await?,
        other => {
            return Err(OidcError::UnsupportedGrantType(format!(
                "grant_type '{other}' is not supported"
            )))
        }
    };
    // RFC 6749 §5.1: token responses MUST include Cache-Control: no-store.
    Ok((TOKEN_HEADERS, json))
}

async fn handle_password_grant(
    state: &OidcState,
    req: TokenRequest,
) -> Result<Json<TokenResponse>, OidcError> {
    if !state.config.password_grant_enabled {
        return Err(OidcError::UnsupportedGrantType(
            "password grant is disabled; enable it only for development fixtures".into(),
        ));
    }

    let username = req
        .username
        .ok_or_else(|| OidcError::InvalidRequest("missing 'username' parameter".into()))?;
    let password = req
        .password
        .ok_or_else(|| OidcError::InvalidRequest("missing 'password' parameter".into()))?;

    debug!(%username, "Processing password grant");

    let _permit = state
        .credential_verification_limiter
        .acquire()
        .await
        .map_err(|_| OidcError::Internal("credential verification limiter closed".into()))?;

    let user = state
        .user_store
        .authenticate(&username, &password)
        .await
        .map_err(|e| {
            warn!(error = %e, "User store authentication failed");
            OidcError::Internal("user store authentication failed".into())
        })?;

    let Some(user) = user else {
        debug!(%username, "Invalid credentials");
        return Err(OidcError::InvalidGrant(
            "invalid username or password".into(),
        ));
    };

    let scope = normalize_scope(req.scope.as_deref(), DEFAULT_USER_SCOPE);
    let token = state.token_service.issue_user_token(&user, &scope)?;

    Ok(Json(TokenResponse {
        access_token: token,
        token_type: "Bearer",
        expires_in: state.token_service.token_ttl_secs(),
    }))
}

async fn handle_client_credentials_grant(
    state: &OidcState,
    headers: &HeaderMap,
    req: &TokenRequest,
) -> Result<Json<TokenResponse>, OidcError> {
    if state.client_registry.is_empty() {
        return Err(OidcError::UnsupportedGrantType(
            "client_credentials grant is not configured".into(),
        ));
    }

    let body_credentials = req.client_id.as_deref().zip(req.client_secret.as_deref());
    let basic_credentials = extract_basic_client_credentials(headers)?;
    if body_credentials.is_some() && basic_credentials.is_some() {
        return Err(OidcError::InvalidRequest(
            "client credentials must use exactly one authentication method".into(),
        ));
    }

    let credentials = match (basic_credentials, body_credentials) {
        (Some(credentials), None) => credentials,
        (None, Some((client_id, client_secret))) => {
            (client_id.to_string(), client_secret.to_string())
        }
        (None, None) => {
            return Err(OidcError::InvalidClient(
                "missing client authentication".into(),
            ))
        }
        (Some(_), Some(_)) => unreachable!("checked above"),
    };
    let (client_id, client_secret) = credentials;

    debug!(client_id, "Processing client_credentials grant");

    let _permit = state
        .credential_verification_limiter
        .acquire()
        .await
        .map_err(|_| OidcError::Internal("credential verification limiter closed".into()))?;

    if !state
        .client_registry
        .validate(&client_id, &client_secret)
        .await
        .map_err(|e| {
            warn!(error = %e, "Client registry validation failed");
            OidcError::Internal("client registry validation failed".into())
        })?
    {
        debug!(client_id, "Invalid client credentials");
        return Err(OidcError::InvalidClient(
            "invalid client credentials".into(),
        ));
    }

    let scope = normalize_scope(req.scope.as_deref(), "");
    let token = state.token_service.issue_client_token(&client_id, &scope)?;

    Ok(Json(TokenResponse {
        access_token: token,
        token_type: "Bearer",
        expires_in: state.token_service.token_ttl_secs(),
    }))
}

/// GET /.well-known/openid-configuration
pub(crate) async fn discovery_handler(State(state): State<Arc<OidcState>>) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        state.discovery_json.to_string(),
    )
}

/// GET /.well-known/jwks.json
pub(crate) async fn jwks_handler(State(state): State<Arc<OidcState>>) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        state.jwks_json.to_string(),
    )
}

/// Userinfo response.
#[derive(Serialize)]
pub(crate) struct UserinfoResponse {
    sub: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    roles: Vec<String>,
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

/// GET /userinfo
pub(crate) async fn userinfo_handler(
    State(state): State<Arc<OidcState>>,
    headers: HeaderMap,
) -> Result<Json<UserinfoResponse>, OidcError> {
    let token = extract_bearer_token(&headers)?;

    let claims = state
        .claims_validator
        .validate_as::<AccessTokenClaims>(token)
        .await
        .map_err(|e| {
            debug!(error = %e, "Userinfo token validation failed");
            OidcError::InvalidToken("invalid access token".into())
        })?;

    if claims.token_use != "access" || claims.principal_type != "user" {
        return Err(OidcError::InvalidToken(
            "userinfo requires a user access token".into(),
        ));
    }

    if !has_scope(&claims.scope, "openid") {
        return Err(OidcError::InsufficientScope(
            "userinfo requires the 'openid' scope".into(),
        ));
    }

    let user = state
        .user_store
        .find_by_sub(&claims.sub)
        .await
        .map_err(|e| {
            warn!(error = %e, "User store lookup failed");
            OidcError::Internal("user store lookup failed".into())
        })?
        .ok_or_else(|| OidcError::InvalidToken("user not found".into()))?;

    Ok(Json(UserinfoResponse {
        sub: user.sub,
        email: user.email,
        roles: user.roles,
        extra: crate::token::filter_extra_claims(&user.extra_claims),
    }))
}

fn extract_bearer_token(headers: &HeaderMap) -> Result<&str, OidcError> {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| OidcError::Unauthorized("missing Authorization header".into()))?;

    r2e_security::extractor::extract_bearer_token(auth)
        .map_err(|_| OidcError::Unauthorized("expected Bearer token".into()))
}

fn extract_basic_client_credentials(
    headers: &HeaderMap,
) -> Result<Option<(String, String)>, OidcError> {
    let Some(auth) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return Ok(None);
    };

    let Some((scheme, encoded)) = auth.split_once(' ') else {
        return Ok(None);
    };

    if !scheme.eq_ignore_ascii_case("Basic") {
        return Ok(None);
    }

    let decoded = STANDARD
        .decode(encoded.trim())
        .map_err(|_| OidcError::InvalidClient("invalid Basic client authentication".into()))?;
    let decoded = String::from_utf8(decoded)
        .map_err(|_| OidcError::InvalidClient("invalid Basic client authentication".into()))?;
    let (client_id, client_secret) = decoded
        .split_once(':')
        .ok_or_else(|| OidcError::InvalidClient("invalid Basic client authentication".into()))?;

    if client_id.is_empty() || client_secret.is_empty() {
        return Err(OidcError::InvalidClient(
            "invalid Basic client authentication".into(),
        ));
    }

    Ok(Some((client_id.to_string(), client_secret.to_string())))
}
