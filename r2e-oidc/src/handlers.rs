use std::sync::Arc;

use axum::extract::State;
use axum::http::header;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::Form;
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::OidcError;
use crate::state::OidcState;

/// RFC 6749 §5.1 required headers for token responses.
type TokenResponseHeaders = [(header::HeaderName, &'static str); 2];
const TOKEN_HEADERS: TokenResponseHeaders = [
    (header::CACHE_CONTROL, "no-store"),
    (header::PRAGMA, "no-cache"),
];

/// Token request parameters (form-urlencoded).
#[derive(Debug, Deserialize)]
pub(crate) struct TokenRequest {
    pub grant_type: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
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
    Form(req): Form<TokenRequest>,
) -> Result<impl IntoResponse, OidcError> {
    let json = match req.grant_type.as_str() {
        "password" => handle_password_grant(&state, req).await?,
        "client_credentials" => handle_client_credentials_grant(&state, &req).await?,
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
    let username = req
        .username
        .ok_or_else(|| OidcError::InvalidRequest("missing 'username' parameter".into()))?;
    let password = req
        .password
        .ok_or_else(|| OidcError::InvalidRequest("missing 'password' parameter".into()))?;

    debug!(%username, "Processing password grant");

    let valid = state
        .user_store
        .verify_password(&username, &password)
        .await;
    if !valid {
        warn!(%username, "Invalid credentials");
        return Err(OidcError::InvalidGrant(
            "invalid username or password".into(),
        ));
    }

    let user = state
        .user_store
        .find_by_username(&username)
        .await
        .ok_or_else(|| OidcError::InvalidGrant("user not found".into()))?;

    let token = state.token_service.issue_token(&user)?;

    Ok(Json(TokenResponse {
        access_token: token,
        token_type: "Bearer",
        expires_in: state.token_service.token_ttl_secs(),
    }))
}

async fn handle_client_credentials_grant(
    state: &OidcState,
    req: &TokenRequest,
) -> Result<Json<TokenResponse>, OidcError> {
    if state.client_registry.is_empty() {
        return Err(OidcError::UnsupportedGrantType(
            "client_credentials grant is not configured".into(),
        ));
    }

    let client_id = req
        .client_id
        .as_deref()
        .ok_or_else(|| OidcError::InvalidRequest("missing 'client_id' parameter".into()))?;
    let client_secret = req
        .client_secret
        .as_deref()
        .ok_or_else(|| OidcError::InvalidRequest("missing 'client_secret' parameter".into()))?;

    debug!(client_id, "Processing client_credentials grant");

    if !state.client_registry.validate(client_id, client_secret).await {
        warn!(client_id, "Invalid client credentials");
        return Err(OidcError::InvalidClient(
            "invalid client credentials".into(),
        ));
    }

    let token = state.token_service.issue_client_token(client_id)?;

    Ok(Json(TokenResponse {
        access_token: token,
        token_type: "Bearer",
        expires_in: state.token_service.token_ttl_secs(),
    }))
}

/// OpenID Connect discovery document.
#[derive(Serialize)]
pub(crate) struct DiscoveryDocument {
    issuer: String,
    token_endpoint: String,
    jwks_uri: String,
    userinfo_endpoint: String,
    grant_types_supported: Vec<&'static str>,
    subject_types_supported: Vec<&'static str>,
    id_token_signing_alg_values_supported: Vec<&'static str>,
    response_types_supported: Vec<&'static str>,
}

/// GET /.well-known/openid-configuration
pub(crate) async fn discovery_handler(
    State(state): State<Arc<OidcState>>,
) -> Json<DiscoveryDocument> {
    let base = format!("{}{}", state.config.issuer, state.config.base_path);
    Json(DiscoveryDocument {
        issuer: state.config.issuer.clone(),
        token_endpoint: format!("{base}/oauth/token"),
        jwks_uri: format!("{base}/.well-known/jwks.json"),
        userinfo_endpoint: format!("{base}/userinfo"),
        grant_types_supported: vec!["password", "client_credentials"],
        subject_types_supported: vec!["public"],
        id_token_signing_alg_values_supported: vec!["RS256"],
        response_types_supported: vec!["token"],
    })
}

/// GET /.well-known/jwks.json
pub(crate) async fn jwks_handler(
    State(state): State<Arc<OidcState>>,
) -> impl IntoResponse {
    Json(state.jwks_json_value())
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
        .validate(token)
        .await
        .map_err(|e| OidcError::Unauthorized(format!("invalid token: {e}")))?;

    let sub = claims
        .get("sub")
        .and_then(|v| v.as_str())
        .ok_or_else(|| OidcError::Unauthorized("token missing 'sub' claim".into()))?;

    let user = state
        .user_store
        .find_by_sub(sub)
        .await
        .ok_or_else(|| OidcError::Unauthorized("user not found".into()))?;

    Ok(Json(UserinfoResponse {
        sub: user.sub,
        email: user.email,
        roles: user.roles,
        extra: user.extra_claims,
    }))
}

fn extract_bearer_token<'a>(headers: &'a HeaderMap) -> Result<&'a str, OidcError> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| OidcError::Unauthorized("missing Authorization header".into()))?;

    auth.strip_prefix("Bearer ")
        .ok_or_else(|| OidcError::Unauthorized("expected Bearer token".into()))
}
