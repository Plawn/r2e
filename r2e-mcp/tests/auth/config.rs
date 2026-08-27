//! `mcp.auth.*` boot validation and canonical-resource resolution.
//!
//! Boot failures surface as panics from `build_state()`
//! ("Plugin 'mcp' failed to build: …"), asserted via `should_panic`; the
//! resolved resource is observed through the PRM document it feeds.

use r2e_core::http::StatusCode;
use r2e_core::AppBuilder;
use r2e_mcp::auth::{AudienceMode, McpAuthConfig, TokenValidationMode};
use r2e_mcp::{AppBuilderMcpExt, McpServer};
use serde_json::Value;

use crate::fixtures::{self, offline_auth, pinned, secured_app_with, test_jwt, ISSUER};

// ── Boot rejections ────────────────────────────────────────────────────────

#[tokio::test]
#[should_panic(expected = "mcp.auth.issuer must not be empty")]
async fn empty_issuer_is_a_boot_error() {
    let _ = secured_app_with(McpAuthConfig {
        issuer: "   ".to_string(),
        ..offline_auth()
    })
    .await;
}

#[tokio::test]
#[should_panic(expected = "is not https")]
async fn plaintext_issuer_without_allow_insecure_is_a_boot_error() {
    let _ = secured_app_with(McpAuthConfig {
        allow_insecure: None,
        ..offline_auth()
    })
    .await;
}

#[tokio::test]
#[should_panic(expected = "introspection requires a confidential client")]
async fn introspection_without_client_credentials_is_a_boot_error() {
    let _ = secured_app_with(McpAuthConfig {
        token_validation: Some(TokenValidationMode::Introspection),
        ..offline_auth()
    })
    .await;
}

#[tokio::test]
#[should_panic(expected = "an explicit `mcp.auth.introspection-endpoint`")]
async fn offline_introspection_requires_an_explicit_endpoint() {
    let _ = secured_app_with(McpAuthConfig {
        token_validation: Some(TokenValidationMode::Introspection),
        client_id: Some("rs-client".to_string()),
        client_secret: Some("rs-secret".to_string()),
        ..offline_auth()
    })
    .await;
}

#[tokio::test]
#[should_panic(expected = "an explicit `mcp.auth.userinfo-endpoint`")]
async fn offline_userinfo_requires_an_explicit_endpoint() {
    let _ = secured_app_with(McpAuthConfig {
        token_validation: Some(TokenValidationMode::Userinfo),
        ..offline_auth()
    })
    .await;
}

#[tokio::test]
#[should_panic(expected = "requires the DCR shim")]
async fn extra_authorize_params_without_the_shim_is_a_boot_error() {
    // Without the shim no mirrored metadata advertises the redirect
    // endpoint — the params would silently do nothing.
    let _ = secured_app_with(McpAuthConfig {
        extra_authorize_params: Some(
            [("audience".to_string(), "https://api.example".to_string())]
                .into_iter()
                .collect(),
        ),
        ..offline_auth()
    })
    .await;
}

#[tokio::test]
#[should_panic(expected = "cannot be enforced with")]
async fn userinfo_with_an_audience_binding_is_a_boot_error() {
    let _ = secured_app_with(McpAuthConfig {
        token_validation: Some(TokenValidationMode::Userinfo),
        audience: Some(AudienceMode::Resource),
        userinfo_endpoint: Some("http://127.0.0.1:1/userinfo".to_string()),
        ..offline_auth()
    })
    .await;
}

#[tokio::test]
async fn opaque_validation_modes_boot_with_their_endpoints_configured() {
    use crate::fixtures::get;
    for auth in [
        McpAuthConfig {
            token_validation: Some(TokenValidationMode::Introspection),
            client_id: Some("rs-client".to_string()),
            client_secret: Some("rs-secret".to_string()),
            introspection_endpoint: Some("http://127.0.0.1:1/introspect".to_string()),
            ..offline_auth()
        },
        // Userinfo with `audience` unset: skip is forced, boot succeeds.
        McpAuthConfig {
            token_validation: Some(TokenValidationMode::Userinfo),
            userinfo_endpoint: Some("http://127.0.0.1:1/userinfo".to_string()),
            ..offline_auth()
        },
    ] {
        let app = secured_app_with(auth).await;
        // The auth layer is live: an unauthenticated MCP request is a 401.
        let (status, _, _) = get(&app, "/mcp", &[]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
#[should_panic(expected = "`client-id` requires `mcp.auth.public-client-id`")]
async fn client_id_audience_requires_public_client_id() {
    let _ = secured_app_with(McpAuthConfig {
        audience: Some(AudienceMode::ClientId),
        ..offline_auth()
    })
    .await;
}

#[tokio::test]
#[should_panic(expected = "unknown algorithm `HS999`")]
async fn unknown_algorithm_is_a_boot_error() {
    let _ = secured_app_with(McpAuthConfig {
        allowed_algorithms: Some(vec!["RS256".into(), "HS999".into()]),
        ..offline_auth()
    })
    .await;
}

#[tokio::test]
#[should_panic(expected = "requires `mcp.auth.public-client-id`")]
async fn forced_shim_requires_public_client_id() {
    let _ = secured_app_with(McpAuthConfig {
        shim: Some(true),
        ..offline_auth()
    })
    .await;
}

#[tokio::test]
#[should_panic(expected = "mcp.auth.registration-path must start with '/'")]
async fn relative_registration_path_is_a_boot_error() {
    let _ = secured_app_with(McpAuthConfig {
        public_client_id: Some("mcp-public".into()),
        registration_path: Some("oauth/register".into()),
        ..offline_auth()
    })
    .await;
}

#[tokio::test]
#[should_panic(expected = "requires an explicit `mcp.auth.jwks-url`")]
async fn discovery_off_requires_jwks_url() {
    let _ = secured_app_with(McpAuthConfig {
        jwks_url: None,
        ..offline_auth()
    })
    .await;
}

#[tokio::test]
#[should_panic(expected = "OAuth discovery failed at boot")]
async fn eager_discovery_failure_is_a_boot_error() {
    // Eager (the non-dev default) probes the issuer at build; a dead one
    // (connection refused, no DNS) must abort startup with a pointer at
    // `discovery: lazy`.
    let _ = secured_app_with(McpAuthConfig {
        issuer: "http://127.0.0.1:1".to_string(),
        discovery: None,
        ..offline_auth()
    })
    .await;
}

// ── Resource resolution ────────────────────────────────────────────────────

async fn prm_resource(router: &r2e_core::http::Router) -> Value {
    let (status, _, body) =
        fixtures::get(router, "/.well-known/oauth-protected-resource", &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_str::<Value>(&body).unwrap()["resource"].clone()
}

/// Explicit `resource` wins and is canonicalised: scheme/host lowercased,
/// default port dropped, query/fragment stripped, trailing slash trimmed.
#[tokio::test]
async fn explicit_resource_is_canonicalized() {
    let router = secured_app_with(McpAuthConfig {
        resource: Some("http://LOCALHOST:80/mcp/?q=1#frag".to_string()),
        ..offline_auth()
    })
    .await;
    assert_eq!(prm_resource(&router).await, "http://localhost/mcp");
}

fn config_boot(auth_overrides: &[(&str, &str)]) -> AppBuilder {
    let mut builder = AppBuilder::new()
        .override_config_value("mcp.auth.issuer", ISSUER)
        .override_config_value("mcp.auth.allow-insecure", true)
        .override_config_value("mcp.auth.discovery", "off")
        .override_config_value("mcp.auth.jwks-url", fixtures::DEAD_JWKS);
    for (key, value) in auth_overrides {
        builder = builder.override_config_value(*key, *value);
    }
    builder
}

/// No explicit resource: `{server.public-url}{mcp.path}` is next in line.
#[tokio::test]
async fn resource_derives_from_public_url() {
    let router = config_boot(&[("server.public-url", "https://api.example.com/")])
        .load_config::<()>()
        .plugin(McpServer::new().with_token_validator(pinned(&test_jwt())))
        .build_state()
        .await
        .register_mcp_service::<fixtures::SecuredTools>()
        .build();
    assert_eq!(prm_resource(&router).await, "https://api.example.com/mcp");
}

/// Loopback bind without `server.public-url`: the dev fallback derives the
/// resource from `server.host`/`server.port`.
#[tokio::test]
async fn resource_falls_back_to_loopback_bind() {
    let router = config_boot(&[("server.host", "127.0.0.1")])
        .override_config_value("server.port", 8081u16)
        .load_config::<()>()
        .plugin(McpServer::new().with_token_validator(pinned(&test_jwt())))
        .build_state()
        .await
        .register_mcp_service::<fixtures::SecuredTools>()
        .build();
    assert_eq!(prm_resource(&router).await, "http://127.0.0.1:8081/mcp");
}

/// Non-loopback bind, no `server.public-url`, non-dev profile: refusing to
/// guess beats advertising a wrong resource URI.
#[tokio::test]
#[should_panic(expected = "cannot determine the canonical resource URI")]
async fn unresolvable_resource_is_a_boot_error() {
    let _ = config_boot(&[("server.host", "10.0.0.5")])
        .load_config::<()>()
        .plugin(McpServer::new().with_token_validator(pinned(&test_jwt())))
        .build_state()
        .await
        .register_mcp_service::<fixtures::SecuredTools>()
        .build();
}
