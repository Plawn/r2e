//! Boot-time assembly of the auth surface — the plugin's `build` hands the
//! resolved `mcp.auth` section to [`build_auth`] and gets back everything it
//! mounts: the tower layer, the provided validator bean, the `OnceLock` slot
//! the layer reads it through, and the well-known/shim routes.
//!
//! Nothing here performs network I/O except the optional eager discovery
//! probe at the very end (`discovery: eager`, the non-dev default) — the
//! validator itself is ALWAYS lazily initialised on the first validated
//! request ([`McpTokenValidator::lazy_jwt`]), because building a
//! `JwksCache` fetches keys and boot must stay offline-safe under a pinned
//! validator.

use std::str::FromStr;
use std::sync::{Arc, OnceLock};

use r2e_core::http::Router;
use r2e_core::plugin::PluginBuildError;
use r2e_security::{Algorithm, SecurityConfig};

use super::config::{AudienceMode, DiscoveryMode, McpAuthConfig, TokenValidationMode};
use super::discovery::{DiscoveryClient, OAuthServerMetadata};
use super::layer::{origin_allowed, AuthState, McpAuthLayer};
use super::opaque::{
    IntrospectionBackend, UserinfoBackend, DEFAULT_OPAQUE_CACHE_MAX_ENTRIES,
    DEFAULT_OPAQUE_CACHE_TTL_SECS,
};
use super::shim::{shim_routes, ShimState, DEFAULT_REDIRECT_ALLOWLIST};
use super::validator::{McpTokenValidator, ScopePolicy};
use super::wellknown::{prm_json, prm_routes};

/// Everything the plugin's `build` knows that the auth assembly needs.
pub(crate) struct AuthInputs<'a> {
    pub cfg: McpAuthConfig,
    /// The endpoint path (`mcp.path`, validated).
    pub mcp_path: &'a str,
    /// Active config profile (`""` when no config was loaded).
    pub profile: &'a str,
    /// `server.public-url` when configured.
    pub public_url: Option<String>,
    /// `server.host` / `server.port` (the dev fallback for `resource`).
    pub server_host: Option<String>,
    pub server_port: Option<u16>,
    /// `mcp.allowed-origins` (the transport-level Origin check, reused by
    /// the auth layer's DNS-rebinding guard).
    pub allowed_origins: &'a [String],
    /// `McpServer::with_token_validator` override.
    pub validator_override: Option<McpTokenValidator>,
}

/// What [`build_auth`] hands back to the plugin.
pub(crate) struct AuthArtifacts {
    /// The tower layer to wrap the MCP service with.
    pub layer: McpAuthLayer,
    /// The validator to expose as the provided bean.
    pub validator: McpTokenValidator,
    /// The slot the layer reads the validator through; the plugin fills it
    /// from the bean context in `after_build` so a test-pinned validator is
    /// RESOLVED, not captured.
    pub slot: Arc<OnceLock<McpTokenValidator>>,
    /// PRM + (optionally) shim routes, merged NEXT TO the MCP service —
    /// never behind the auth layer.
    pub extra_routes: Router,
}

/// Canonicalise a resource URI (RFC 8707): `url::Url` lowercases the
/// scheme/host and drops the default port; query/fragment are stripped and
/// the trailing slash dropped.
fn canonicalize_resource(input: &str) -> Result<String, PluginBuildError> {
    let mut url = url::Url::parse(input)
        .map_err(|e| format!("mcp.auth: resource URI `{input}` is not a valid URL: {e}"))?;
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string().trim_end_matches('/').to_string())
}

/// Resolve the canonical resource URI:
/// `mcp.auth.resource` → `{server.public-url}{mcp.path}` → (dev/test profile
/// or loopback bind only) `http://{host}:{port}{mcp.path}` with a `warn!` →
/// boot error naming both keys.
fn resolve_resource(inputs: &AuthInputs<'_>) -> Result<String, PluginBuildError> {
    if let Some(resource) = &inputs.cfg.resource {
        return canonicalize_resource(resource);
    }
    if let Some(base) = &inputs.public_url {
        return canonicalize_resource(&format!(
            "{}{}",
            base.trim_end_matches('/'),
            inputs.mcp_path
        ));
    }
    let host = inputs.server_host.as_deref().unwrap_or("0.0.0.0");
    let dev_like = matches!(inputs.profile, "dev" | "test");
    let local_bind = matches!(host, "127.0.0.1" | "::1" | "localhost" | "0.0.0.0" | "::");
    if dev_like || local_bind {
        let url_host = if matches!(host, "0.0.0.0" | "::") {
            "localhost"
        } else {
            host
        };
        let port = inputs.server_port.unwrap_or(3000);
        let derived = format!("http://{url_host}:{port}{}", inputs.mcp_path);
        tracing::warn!(
            resource = %derived,
            "mcp.auth.resource derived from the bind address — fine for local \
             development; set `server.public-url` (or `mcp.auth.resource`) for \
             any deployment clients reach through a public URL"
        );
        return canonicalize_resource(&derived);
    }
    Err(format!(
        "mcp.auth: cannot determine the canonical resource URI of the MCP \
         endpoint (bound to non-loopback host `{host}`). Set `server.public-url` \
         (recommended) or `mcp.auth.resource` explicitly"
    )
    .into())
}

/// `{origin}` and `{path}` halves of a canonical resource URI. The path is
/// `""` for an origin-only resource.
fn split_resource(resource: &str) -> Result<(String, String), PluginBuildError> {
    let url = url::Url::parse(resource)
        .map_err(|e| format!("mcp.auth: resource URI `{resource}` is not a valid URL: {e}"))?;
    let origin = url.origin().ascii_serialization();
    let path = match url.path() {
        "/" => String::new(),
        p => p.to_string(),
    };
    Ok((origin, path))
}

/// Assemble the whole auth surface from the resolved `mcp.auth` section.
pub(crate) async fn build_auth(inputs: AuthInputs<'_>) -> Result<AuthArtifacts, PluginBuildError> {
    let cfg = &inputs.cfg;

    let validation_mode = cfg.token_validation.unwrap_or_default();
    if validation_mode == TokenValidationMode::Introspection
        && (cfg.client_id.is_none() || cfg.client_secret.is_none())
    {
        return Err(
            "mcp.auth.token-validation: introspection requires a confidential client — \
             set `mcp.auth.client-id` and `mcp.auth.client-secret`"
                .to_string()
                .into(),
        );
    }
    if validation_mode == TokenValidationMode::Userinfo {
        match cfg.audience {
            // Forced skip: the userinfo response carries no `aud` to check.
            None | Some(AudienceMode::Skip) => tracing::warn!(
                "mcp.auth.token-validation: userinfo — the userinfo endpoint cannot bind \
                 a token to an audience, so `aud` is NOT validated; any live token minted \
                 by the issuer authenticates here"
            ),
            Some(other) => {
                return Err(format!(
                    "mcp.auth.audience: `{other:?}` cannot be enforced with \
                     `token-validation: userinfo` (the userinfo endpoint returns no \
                     audience) — remove the key or set `audience: skip` explicitly"
                )
                .into());
            }
        }
    }

    let allow_insecure = cfg.allow_insecure.unwrap_or(false);
    let issuer = cfg.issuer.trim().to_string();
    if issuer.is_empty() {
        return Err("mcp.auth.issuer must not be empty".into());
    }
    // NOTE: the issuer is kept EXACTLY as configured (Auth0 issuers carry a
    // trailing slash that must survive into the `iss` claim check); only
    // discovery comparisons normalise it.
    if !issuer.starts_with("https://") && !allow_insecure {
        return Err(format!(
            "mcp.auth.issuer `{issuer}` is not https — a plaintext issuer lets a \
             network MITM substitute signing keys. Set `mcp.auth.allow-insecure: true` \
             for local development only"
        )
        .into());
    }

    let resource = resolve_resource(&inputs)?;
    let (resource_origin, resource_path) = split_resource(&resource)?;
    let resource_metadata_url =
        format!("{resource_origin}/.well-known/oauth-protected-resource{resource_path}");

    // ── SecurityConfig (jwks_url may legitimately be empty: the lazy JWT
    // backend resolves it from discovery on first use) ──────────────────────
    let mut sec = SecurityConfig::new(
        cfg.jwks_url.clone().unwrap_or_default(),
        issuer.clone(),
        resource.clone(),
    )
    .with_leeway(cfg.clock_skew_secs.unwrap_or(60));
    // The accepted `aud` values (`None` = skip): applied to the JWT
    // validator here, and to the introspection backend further down.
    let audience_mode = if validation_mode == TokenValidationMode::Userinfo {
        AudienceMode::Skip // forced (warned above)
    } else {
        cfg.audience.unwrap_or_default()
    };
    let accepted_audiences: Option<Vec<String>> = match audience_mode {
        AudienceMode::Resource => Some(vec![resource.clone()]),
        AudienceMode::AnyOf => {
            let mut audiences = vec![resource.clone()];
            audiences.extend(cfg.extra_audiences.clone().unwrap_or_default());
            Some(audiences)
        }
        AudienceMode::ClientId => {
            let client_id = cfg.public_client_id.clone().ok_or_else(|| {
                PluginBuildError::from(
                    "mcp.auth.audience: `client-id` requires `mcp.auth.public-client-id`"
                        .to_string(),
                )
            })?;
            Some(vec![client_id])
        }
        AudienceMode::Skip => {
            if validation_mode != TokenValidationMode::Userinfo {
                tracing::warn!(
                    "mcp.auth.audience: skip — the `aud` claim is NOT validated; any token \
                     minted by the issuer for any service will authenticate here"
                );
            }
            None
        }
    };
    sec = match &accepted_audiences {
        Some(audiences) => sec.with_audiences(audiences.clone()),
        None => sec.skip_audience_validation(),
    };
    let algorithms: Vec<Algorithm> = match &cfg.allowed_algorithms {
        Some(names) => names
            .iter()
            .map(|name| {
                Algorithm::from_str(name).map_err(|_| {
                    PluginBuildError::from(format!(
                        "mcp.auth.allowed-algorithms: unknown algorithm `{name}`"
                    ))
                })
            })
            .collect::<Result<_, _>>()?,
        // Asymmetric-only default: an HS* algorithm against a JWKS makes no
        // sense and inviting it invites key-confusion bugs.
        None => vec![Algorithm::RS256, Algorithm::ES256, Algorithm::PS256],
    };
    sec = sec.with_allowed_algorithms(algorithms);
    if allow_insecure {
        sec = sec.allow_insecure_jwks_url();
    }

    // ── Discovery ───────────────────────────────────────────────────────────
    let discovery_mode = cfg.discovery.unwrap_or(if inputs.profile == "dev" {
        DiscoveryMode::Lazy
    } else {
        DiscoveryMode::Eager
    });
    let discovery = if discovery_mode == DiscoveryMode::Off {
        let missing = match validation_mode {
            TokenValidationMode::Jwt if cfg.jwks_url.is_none() => Some("mcp.auth.jwks-url"),
            TokenValidationMode::Introspection if cfg.introspection_endpoint.is_none() => {
                Some("mcp.auth.introspection-endpoint")
            }
            TokenValidationMode::Userinfo if cfg.userinfo_endpoint.is_none() => {
                Some("mcp.auth.userinfo-endpoint")
            }
            _ => None,
        };
        if let Some(key) = missing {
            return Err(format!(
                "mcp.auth: `discovery: off` with `{validation_mode:?}` validation requires \
                 an explicit `{key}`"
            )
            .into());
        }
        Arc::new(DiscoveryClient::fixed(OAuthServerMetadata::from_endpoints(
            issuer.clone(),
            cfg.jwks_url.clone(),
            cfg.authorization_endpoint.clone(),
            cfg.token_endpoint.clone(),
            cfg.registration_endpoint.clone(),
            cfg.userinfo_endpoint.clone(),
            cfg.introspection_endpoint.clone(),
        )))
    } else {
        let client = r2e_security::build_oauth_http_client(&sec)
            .map_err(|e| format!("mcp.auth: failed to build the OAuth HTTP client: {e}"))?;
        Arc::new(DiscoveryClient::new(
            client,
            issuer.clone(),
            cfg.discovery_ttl_secs.unwrap_or(3600),
        ))
    };
    if discovery_mode == DiscoveryMode::Eager {
        // Boot-time validation of the IdP config only — the result is cached,
        // but the validator stays lazy either way.
        discovery.get().await.map_err(|e| {
            PluginBuildError::from(format!(
                "mcp.auth: OAuth discovery failed at boot for issuer `{issuer}`: {e}. \
                 Use `mcp.auth.discovery: lazy` if the IdP starts after the app"
            ))
        })?;
    }

    // ── Validator (the provided bean) ───────────────────────────────────────
    let scope_policy = ScopePolicy {
        scope_claim: cfg.scope_claim.clone(),
        roles_claim: cfg.roles_claim.clone(),
        client_roles_for: cfg.client_roles_for.clone(),
    };
    let opaque_cache_ttl =
        std::time::Duration::from_secs(cfg.opaque_cache_ttl_secs.unwrap_or(DEFAULT_OPAQUE_CACHE_TTL_SECS));
    let opaque_cache_max = cfg
        .opaque_cache_max_entries
        .unwrap_or(DEFAULT_OPAQUE_CACHE_MAX_ENTRIES);
    let validator = match inputs.validator_override.clone() {
        Some(validator) => validator,
        None => match validation_mode {
            TokenValidationMode::Jwt => {
                McpTokenValidator::lazy_jwt(sec, Some(discovery.clone()), scope_policy)
            }
            TokenValidationMode::Introspection => {
                let client = r2e_security::build_oauth_http_client(&sec)
                    .map_err(|e| format!("mcp.auth: failed to build the OAuth HTTP client: {e}"))?;
                let mut backend = IntrospectionBackend::new(
                    client,
                    discovery.clone(),
                    cfg.client_id.clone().expect("checked above"),
                    cfg.client_secret.clone().expect("checked above"),
                )
                .with_leeway(cfg.clock_skew_secs.unwrap_or(60))
                .with_scope_policy(scope_policy)
                .with_cache(opaque_cache_ttl, opaque_cache_max);
                if let Some(endpoint) = cfg.introspection_endpoint.clone() {
                    backend = backend.with_endpoint(endpoint);
                }
                if let Some(audiences) = accepted_audiences.clone() {
                    backend = backend.with_audiences(audiences);
                }
                McpTokenValidator::custom(backend)
            }
            TokenValidationMode::Userinfo => {
                let client = r2e_security::build_oauth_http_client(&sec)
                    .map_err(|e| format!("mcp.auth: failed to build the OAuth HTTP client: {e}"))?;
                let mut backend = UserinfoBackend::new(client, discovery.clone())
                    .with_scope_policy(scope_policy)
                    .with_cache(opaque_cache_ttl, opaque_cache_max);
                if let Some(endpoint) = cfg.userinfo_endpoint.clone() {
                    backend = backend.with_endpoint(endpoint);
                }
                McpTokenValidator::custom(backend)
            }
        },
    };

    // ── Layer state ─────────────────────────────────────────────────────────
    let slot: Arc<OnceLock<McpTokenValidator>> = Arc::new(OnceLock::new());
    let required_scopes: Arc<[String]> = cfg.required_scopes.clone().unwrap_or_default().into();
    let layer = McpAuthLayer::enabled(AuthState {
        validator: slot.clone(),
        resource_metadata_url: resource_metadata_url.clone().into(),
        required_scopes,
        allowed_origins: if inputs.allowed_origins.is_empty() {
            None
        } else {
            Some(inputs.allowed_origins.to_vec().into())
        },
    });

    // ── Well-known routes (PRM + optional DCR shim) ─────────────────────────
    let shim_on = cfg.shim.unwrap_or(cfg.public_client_id.is_some());
    if shim_on && cfg.public_client_id.is_none() {
        return Err(
            "mcp.auth.shim: true requires `mcp.auth.public-client-id` (the client id \
             the shim hands out)"
                .to_string()
                .into(),
        );
    }
    let scopes_supported = cfg
        .scopes_supported
        .clone()
        .or_else(|| cfg.required_scopes.clone())
        .unwrap_or_default();
    // Shim on: clients must fetch the AUTHORIZATION SERVER metadata from us
    // (the mirrored document carrying the rewritten registration endpoint),
    // so the PRM points at the resource origin instead of the issuer.
    let authorization_servers = if shim_on {
        vec![resource_origin.clone()]
    } else {
        vec![issuer.clone()]
    };
    let prm = prm_json(
        &resource,
        &authorization_servers,
        &scopes_supported,
        cfg.resource_name.as_deref(),
    );
    let mut extra_routes = prm_routes(prm, inputs.mcp_path);
    if shim_on {
        let registration_path = cfg
            .registration_path
            .clone()
            .unwrap_or_else(|| "/oauth/register".to_string());
        if !registration_path.starts_with('/') {
            return Err(format!(
                "mcp.auth.registration-path must start with '/': got `{registration_path}`"
            )
            .into());
        }
        let shim_state = Arc::new(ShimState {
            discovery: discovery.clone(),
            client_id: cfg.public_client_id.clone().expect("checked above"),
            registration_endpoint: format!(
                "{resource_origin}{}{registration_path}",
                inputs.mcp_path
            ),
            scopes_supported: scopes_supported.clone(),
            redirect_allowlist: cfg.redirect_uri_allowlist.clone().unwrap_or_else(|| {
                DEFAULT_REDIRECT_ALLOWLIST
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            }),
        });
        extra_routes = extra_routes.merge(shim_routes(shim_state, inputs.mcp_path, &registration_path));
    }

    tracing::info!(
        resource = %resource,
        resource_metadata = %resource_metadata_url,
        authorization_servers = ?authorization_servers,
        shim = shim_on,
        discovery = ?discovery_mode,
        "MCP OAuth resource server enabled"
    );

    Ok(AuthArtifacts {
        layer,
        validator,
        slot,
        extra_routes,
    })
}

/// The CORS layer for the MCP endpoint — applied whether auth is on or off
/// (browser MCP clients need it either way), OUTERMOST so even a 401
/// carries CORS headers and `expose_headers` lets the browser read the
/// `WWW-Authenticate` challenge and the session id.
pub(crate) fn cors_layer(origins: Vec<String>) -> tower_http::cors::CorsLayer {
    use r2e_core::http::header::{HeaderName, AUTHORIZATION, CONTENT_TYPE, WWW_AUTHENTICATE};
    use r2e_core::http::Method;
    use tower_http::cors::{AllowOrigin, CorsLayer};

    let session_id = HeaderName::from_static("mcp-session-id");
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin, _| {
            origin
                .to_str()
                .is_ok_and(|origin| origin_allowed(&origins, origin))
        }))
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        .allow_headers([
            AUTHORIZATION,
            CONTENT_TYPE,
            HeaderName::from_static("mcp-protocol-version"),
            session_id.clone(),
            HeaderName::from_static("last-event-id"),
        ])
        .expose_headers([WWW_AUTHENTICATE, session_id])
}

/// Default `mcp.cors.allowed-origins`: Claude's origins, plus localhost on
/// any port under the `dev` profile.
pub(crate) fn default_cors_origins(profile: &str) -> Vec<String> {
    let mut origins = vec![
        "https://claude.ai".to_string(),
        "https://claude.com".to_string(),
    ];
    if profile == "dev" {
        origins.push("http://localhost:*".to_string());
        origins.push("http://127.0.0.1:*".to_string());
    }
    origins
}
