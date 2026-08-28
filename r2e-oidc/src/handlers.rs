use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine};
use r2e_core::http::extract::Query;
use r2e_core::http::extract::State;
use r2e_core::http::header;
use r2e_core::http::response::{Html, IntoResponse, Redirect, Response};
use r2e_core::http::Form;
use r2e_core::http::HeaderMap;
use r2e_core::http::Json;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::authorization::{valid_code_challenge, verify_s256, AuthorizationGrant};
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
    /// RFC 8707 `resource` indicator. One value only; this embedded issuer
    /// accepts it only when it names the configured audience.
    pub resource: Option<String>,
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_verifier: Option<String>,
}

/// Validate an RFC 8707 `resource` parameter and bind it to this issuer's
/// configured audience. Accepting arbitrary targets would let a shared
/// issuer mint tokens for resources it does not own.
fn validate_resource(resource: Option<&str>, audience: &str) -> Result<(), OidcError> {
    let Some(resource) = resource else {
        return Ok(());
    };
    let valid = url::Url::parse(resource).is_ok_and(|u| u.fragment().is_none());
    if !valid {
        return Err(OidcError::InvalidTarget(format!(
            "'resource' must be an absolute URI without a fragment: got `{resource}`"
        )));
    }
    if resource != audience {
        return Err(OidcError::InvalidTarget(format!(
            "resource `{resource}` is not served by this issuer; expected `{audience}`"
        )));
    }
    Ok(())
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
        "authorization_code" => handle_authorization_code_grant(&state, req)?,
        other => {
            return Err(OidcError::UnsupportedGrantType(format!(
                "grant_type '{other}' is not supported"
            )))
        }
    };
    // RFC 6749 §5.1: token responses MUST include Cache-Control: no-store.
    Ok((TOKEN_HEADERS, json))
}

fn handle_authorization_code_grant(
    state: &OidcState,
    req: TokenRequest,
) -> Result<Json<TokenResponse>, OidcError> {
    if !state.client_registry.has_public_clients() {
        return Err(OidcError::UnsupportedGrantType(
            "authorization_code grant is not configured".into(),
        ));
    }
    let code = req
        .code
        .ok_or_else(|| OidcError::InvalidRequest("missing 'code' parameter".into()))?;
    let client_id = req
        .client_id
        .ok_or_else(|| OidcError::InvalidRequest("missing 'client_id' parameter".into()))?;
    let redirect_uri = req
        .redirect_uri
        .ok_or_else(|| OidcError::InvalidRequest("missing 'redirect_uri' parameter".into()))?;
    let verifier = req
        .code_verifier
        .ok_or_else(|| OidcError::InvalidRequest("missing 'code_verifier' parameter".into()))?;

    let grant = state
        .authorization_codes
        .take(&code)
        .ok_or_else(|| OidcError::InvalidGrant("invalid or expired authorization code".into()))?;
    if grant.client_id != client_id
        || grant.redirect_uri != redirect_uri
        || !state
            .client_registry
            .accepts_redirect(&client_id, &redirect_uri)
        || !verify_s256(&verifier, &grant.code_challenge)
    {
        return Err(OidcError::InvalidGrant(
            "authorization code binding or PKCE verification failed".into(),
        ));
    }
    validate_resource(grant.resource.as_deref(), &state.config.audience)?;
    let token = state
        .token_service
        .issue_user_token(&grant.user, &grant.scope)?;
    Ok(Json(TokenResponse {
        access_token: token,
        token_type: "Bearer",
        expires_in: state.token_service.token_ttl_secs(),
    }))
}

#[derive(Debug, Deserialize)]
pub(crate) struct AuthorizeRequest {
    response_type: Option<String>,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    scope: Option<String>,
    state: Option<String>,
    resource: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AuthorizeForm {
    #[serde(flatten)]
    request: AuthorizeRequest,
    username: String,
    password: String,
}

struct ValidatedAuthorize {
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    scope: String,
    state: Option<String>,
    resource: Option<String>,
}

fn validate_authorize(
    state: &OidcState,
    request: AuthorizeRequest,
) -> Result<ValidatedAuthorize, OidcError> {
    if request.response_type.as_deref() != Some("code") {
        return Err(OidcError::InvalidRequest(
            "response_type must be 'code'".into(),
        ));
    }
    let client_id = request
        .client_id
        .ok_or_else(|| OidcError::InvalidRequest("missing 'client_id' parameter".into()))?;
    let redirect_uri = request
        .redirect_uri
        .ok_or_else(|| OidcError::InvalidRequest("missing 'redirect_uri' parameter".into()))?;
    if !state
        .client_registry
        .accepts_redirect(&client_id, &redirect_uri)
    {
        return Err(OidcError::InvalidRequest(
            "unregistered client or redirect_uri".into(),
        ));
    }
    let code_challenge = request
        .code_challenge
        .filter(|challenge| valid_code_challenge(challenge))
        .ok_or_else(|| OidcError::InvalidRequest("invalid S256 code_challenge".into()))?;
    if request.code_challenge_method.as_deref() != Some("S256") {
        return Err(OidcError::InvalidRequest(
            "code_challenge_method must be 'S256'".into(),
        ));
    }
    validate_resource(request.resource.as_deref(), &state.config.audience)?;
    Ok(ValidatedAuthorize {
        client_id,
        redirect_uri,
        code_challenge,
        scope: normalize_scope(request.scope.as_deref(), DEFAULT_USER_SCOPE),
        state: request.state,
        resource: request.resource,
    })
}

/// GET /oauth/authorize — a deliberately small local login/consent page.
pub(crate) async fn authorize_form_handler(
    State(state): State<Arc<OidcState>>,
    Query(request): Query<AuthorizeRequest>,
) -> Result<Response, OidcError> {
    let request = validate_authorize(&state, request)?;
    let hidden = [
        ("response_type", "code".to_string()),
        ("client_id", request.client_id),
        ("redirect_uri", request.redirect_uri),
        ("code_challenge", request.code_challenge),
        ("code_challenge_method", "S256".to_string()),
        ("scope", request.scope),
    ]
    .into_iter()
    .chain(request.state.map(|value| ("state", value)))
    .chain(request.resource.map(|value| ("resource", value)))
    .map(|(name, value)| {
        format!(
            "<input type=\"hidden\" name=\"{name}\" value=\"{}\">",
            html_escape(&value)
        )
    })
    .collect::<String>();
    let html = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Sign in</title>\
         <main><h1>Sign in</h1><form method=\"post\" action=\"authorize\">{hidden}\
         <label>Username <input name=\"username\" autocomplete=\"username\" required></label>\
         <label>Password <input type=\"password\" name=\"password\" \
         autocomplete=\"current-password\" required></label><button>Authorize</button></form></main>"
    );
    Ok((
        [
            (header::CACHE_CONTROL, "no-store"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'",
            ),
        ],
        Html(html),
    )
        .into_response())
}

/// POST /oauth/authorize — authenticate, mint a one-time code, redirect.
pub(crate) async fn authorize_handler(
    State(state): State<Arc<OidcState>>,
    Form(form): Form<AuthorizeForm>,
) -> Result<Response, OidcError> {
    let request = validate_authorize(&state, form.request)?;
    let _permit = state
        .credential_verification_limiter
        .acquire()
        .await
        .map_err(|_| OidcError::Internal("credential verification limiter closed".into()))?;
    let user = state
        .user_store
        .authenticate(&form.username, &form.password)
        .await
        .map_err(|_| OidcError::Internal("user store authentication failed".into()))?
        .ok_or_else(|| OidcError::InvalidGrant("invalid username or password".into()))?;

    let code = state.authorization_codes.issue(AuthorizationGrant::new(
        request.client_id,
        request.redirect_uri.clone(),
        request.code_challenge,
        request.scope,
        request.resource,
        user,
    ));
    let mut redirect = url::Url::parse(&request.redirect_uri)
        .map_err(|e| OidcError::Internal(format!("registered redirect URI is invalid: {e}")))?;
    redirect.query_pairs_mut().append_pair("code", &code);
    if let Some(value) = request.state {
        redirect.query_pairs_mut().append_pair("state", &value);
    }
    Ok(Redirect::to(redirect.as_str()).into_response())
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
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
    validate_resource(req.resource.as_deref(), &state.config.audience)?;
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
    validate_resource(req.resource.as_deref(), &state.config.audience)?;
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
