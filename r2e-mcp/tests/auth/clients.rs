//! Several pre-registered public clients (`mcp.auth.public-clients`): the
//! DCR shim's per-request client selection, and the validation-side binding
//! of a token to its client (`enforce-azp`, per-client `allowed-scopes`).

use r2e_core::http::{Body, StatusCode};
use r2e_mcp::auth::{AudienceMode, McpAuthConfig, McpPublicClientConfig, TokenValidationMode};
use serde_json::{json, Value};

use crate::fixtures::{
    initialize_auth, offline_auth, post_auth, secured_app_with, send, test_jwt, tool_names,
    tools_call_auth, tools_list_auth,
};
use crate::support;

const REGISTER: &str = "/mcp/oauth/register";
const CLAUDE: &str = "https://claude.ai/api/mcp/auth_callback";

fn saas() -> McpPublicClientConfig {
    McpPublicClientConfig {
        client_id: "mcp-saas".into(),
        redirect_uri_allowlist: vec![
            CLAUDE.into(),
            "https://claude.com/api/mcp/auth_callback".into(),
            "https://dust.tt/*".into(),
        ],
        ..Default::default()
    }
}

fn local_cli() -> McpPublicClientConfig {
    McpPublicClientConfig {
        client_id: "mcp-local-cli".into(),
        redirect_uri_allowlist: vec!["http://127.0.0.1:*".into(), "http://localhost:*".into()],
        allowed_scopes: Some(vec!["mcp".into(), "mcp:read".into()]),
        ..Default::default()
    }
}

fn multi_auth() -> McpAuthConfig {
    McpAuthConfig {
        public_clients: Some(vec![saas(), local_cli()]),
        scopes_supported: Some(vec!["mcp".into(), "mcp:read".into(), "mcp:write".into()]),
        ..offline_auth()
    }
}

async fn register(router: &r2e_core::http::Routes, uris: &[&str]) -> (StatusCode, Value) {
    let body = json!({ "client_name": "agent", "redirect_uris": uris });
    let (status, _, raw) = send(
        router,
        "POST",
        REGISTER,
        &[("content-type", "application/json")],
        Body::from(body.to_string()),
    )
    .await;
    let parsed: Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("not JSON ({e}): {raw}"));
    (status, parsed)
}

/// A token issued to `azp` carrying `scopes`.
fn token_for(azp: &str, scopes: &[&str]) -> String {
    test_jwt()
        .token_builder("alice")
        .scopes(scopes)
        .claim("azp", azp)
        .build()
}

async fn initialize_status(router: &r2e_core::http::Routes, token: &str) -> StatusCode {
    post_auth(router, "/mcp", None, token, &support::initialize_body())
        .await
        .status
}

// ── Shim: client selection ─────────────────────────────────────────────────

#[r2e_core::test]
async fn hosted_callback_gets_the_saas_client() {
    let router = secured_app_with(multi_auth()).await;
    let (status, body) = register(&router, &[CLAUDE, "https://dust.tt/oauth/cb"]).await;

    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["client_id"], "mcp-saas");
    assert_eq!(body["redirect_uris"], json!([CLAUDE, "https://dust.tt/oauth/cb"]));
    // No per-entry scope policy: the advertised scopes, unchanged.
    assert_eq!(body["scope"], "mcp mcp:read mcp:write");
}

#[r2e_core::test]
async fn loopback_redirect_gets_the_local_cli_client() {
    let router = secured_app_with(multi_auth()).await;
    let (status, body) = register(
        &router,
        &["http://127.0.0.1:51234/callback", "http://localhost:8080/cb"],
    )
    .await;

    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["client_id"], "mcp-local-cli");
    // The advertised scopes narrowed to the entry's `allowed-scopes`.
    assert_eq!(body["scope"], "mcp mcp:read");
}

#[r2e_core::test]
async fn explicit_entry_scopes_are_echoed() {
    let router = secured_app_with(McpAuthConfig {
        public_clients: Some(vec![McpPublicClientConfig {
            scopes: Some(vec!["mcp".into()]),
            ..local_cli()
        }]),
        ..multi_auth()
    })
    .await;
    let (_, body) = register(&router, &["http://127.0.0.1:1/cb"]).await;
    assert_eq!(body["scope"], "mcp");
}

#[r2e_core::test]
async fn redirects_split_across_clients_are_refused() {
    let router = secured_app_with(multi_auth()).await;
    let (status, body) = register(&router, &[CLAUDE, "http://127.0.0.1:51234/callback"]).await;

    // Never a silent subset: the caller would hold redirect URIs its
    // client id cannot use.
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "invalid_redirect_uri");
    assert!(
        body["error_description"]
            .as_str()
            .unwrap()
            .contains("different pre-registered clients"),
        "{body}"
    );
}

#[r2e_core::test]
async fn a_redirect_no_client_covers_is_refused() {
    let router = secured_app_with(multi_auth()).await;
    for uris in [
        &["https://evil.example/cb"][..],
        // One unknown URI poisons the whole request (no drop in this form).
        &[CLAUDE, "https://evil.example/cb"][..],
        &[][..],
    ] {
        let (status, body) = register(&router, uris).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uris:?}: {body}");
        assert_eq!(body["error"], "invalid_redirect_uri");
    }
}

#[r2e_core::test]
async fn the_first_covering_entry_wins() {
    let router = secured_app_with(McpAuthConfig {
        public_clients: Some(vec![
            McpPublicClientConfig {
                client_id: "first".into(),
                redirect_uri_allowlist: vec!["http://localhost:*".into()],
                ..Default::default()
            },
            McpPublicClientConfig {
                client_id: "second".into(),
                redirect_uri_allowlist: vec!["http://localhost:*".into()],
                ..Default::default()
            },
        ]),
        ..offline_auth()
    })
    .await;
    let (_, body) = register(&router, &["http://localhost:3000/cb"]).await;
    assert_eq!(body["client_id"], "first");
}

#[r2e_core::test]
async fn the_default_allowlist_does_not_leak_into_entries() {
    // `http://localhost:*` is on the single-client DEFAULT list; an entry
    // answers only for what it lists.
    let router = secured_app_with(McpAuthConfig {
        public_clients: Some(vec![saas()]),
        ..offline_auth()
    })
    .await;
    let (status, _) = register(&router, &["http://localhost:3000/cb"]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ── Boot errors ────────────────────────────────────────────────────────────

#[r2e_core::test]
#[should_panic(expected = "`public-client-id` and `public-clients` are both set")]
async fn both_forms_is_a_boot_error() {
    let _ = secured_app_with(McpAuthConfig {
        public_client_id: Some("mcp-public".into()),
        ..multi_auth()
    })
    .await;
}

#[r2e_core::test]
#[should_panic(expected = "each entry carries its own `redirect-uri-allowlist`")]
async fn top_level_allowlist_with_entries_is_a_boot_error() {
    let _ = secured_app_with(McpAuthConfig {
        redirect_uri_allowlist: Some(vec![CLAUDE.into()]),
        ..multi_auth()
    })
    .await;
}

#[r2e_core::test]
#[should_panic(expected = "`redirect-uri-allowlist` is required")]
async fn an_entry_without_allowlist_is_a_boot_error() {
    let _ = secured_app_with(McpAuthConfig {
        public_clients: Some(vec![McpPublicClientConfig {
            client_id: "mcp-saas".into(),
            ..Default::default()
        }]),
        ..offline_auth()
    })
    .await;
}

#[r2e_core::test]
#[should_panic(expected = "declared twice")]
async fn a_duplicate_client_id_is_a_boot_error() {
    let _ = secured_app_with(McpAuthConfig {
        public_clients: Some(vec![saas(), saas()]),
        ..offline_auth()
    })
    .await;
}

#[r2e_core::test]
#[should_panic(expected = "mcp.auth.public-clients is empty")]
async fn an_empty_list_is_a_boot_error() {
    let _ = secured_app_with(McpAuthConfig {
        public_clients: Some(vec![]),
        ..offline_auth()
    })
    .await;
}

#[r2e_core::test]
#[should_panic(expected = "public-clients[0].allowed-scopes: scope `mcp read` contains a space")]
async fn entry_scopes_are_validated() {
    let _ = secured_app_with(McpAuthConfig {
        public_clients: Some(vec![McpPublicClientConfig {
            allowed_scopes: Some(vec!["mcp read".into()]),
            ..saas()
        }]),
        ..offline_auth()
    })
    .await;
}

#[r2e_core::test]
#[should_panic(expected = "advertised in `scopes` but not in `allowed-scopes`")]
async fn entry_scopes_must_fit_the_allowed_scopes() {
    let _ = secured_app_with(McpAuthConfig {
        public_clients: Some(vec![McpPublicClientConfig {
            scopes: Some(vec!["mcp:write".into()]),
            ..local_cli()
        }]),
        ..offline_auth()
    })
    .await;
}

#[r2e_core::test]
#[should_panic(expected = "enforce-azp: true requires declared clients")]
async fn enforce_azp_without_clients_is_a_boot_error() {
    let _ = secured_app_with(McpAuthConfig {
        enforce_azp: Some(true),
        ..offline_auth()
    })
    .await;
}

#[r2e_core::test]
#[should_panic(expected = "enforce-azp cannot be enforced with `token-validation: userinfo`")]
async fn enforce_azp_with_userinfo_is_a_boot_error() {
    let _ = secured_app_with(McpAuthConfig {
        enforce_azp: Some(true),
        token_validation: Some(TokenValidationMode::Userinfo),
        userinfo_endpoint: Some("http://127.0.0.1:1/userinfo".into()),
        ..multi_auth()
    })
    .await;
}

#[r2e_core::test]
async fn client_id_audience_accepts_the_declared_clients() {
    // Boots: every declared client id is an accepted audience.
    let _ = secured_app_with(McpAuthConfig {
        audience: Some(AudienceMode::ClientId),
        ..multi_auth()
    })
    .await;
}

// ── Validation: enforce-azp ────────────────────────────────────────────────

fn enforcing() -> McpAuthConfig {
    McpAuthConfig {
        enforce_azp: Some(true),
        ..multi_auth()
    }
}

#[r2e_core::test]
async fn enforce_azp_refuses_an_undeclared_client() {
    let router = secured_app_with(enforcing()).await;

    let response = post_auth(
        &router,
        "/mcp",
        None,
        &token_for("some-other-client", &["mcp"]),
        &support::initialize_body(),
    )
    .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    // A token naming no client at all cannot be bound either.
    let anonymous = test_jwt().token("alice", &[]);
    assert_eq!(
        initialize_status(&router, &anonymous).await,
        StatusCode::UNAUTHORIZED
    );
}

#[r2e_core::test]
async fn enforce_azp_admits_declared_clients() {
    let router = secured_app_with(enforcing()).await;
    assert_eq!(
        initialize_status(&router, &token_for("mcp-saas", &["mcp"])).await,
        StatusCode::OK
    );
    assert_eq!(
        initialize_status(&router, &token_for("mcp-local-cli", &["mcp"])).await,
        StatusCode::OK
    );
    // RFC 9068 tokens name the client as `client_id`, not `azp`.
    let rfc9068 = test_jwt()
        .token_builder("alice")
        .claim("client_id", "mcp-saas")
        .build();
    assert_eq!(initialize_status(&router, &rfc9068).await, StatusCode::OK);
}

#[r2e_core::test]
async fn enforce_azp_works_in_the_single_client_form() {
    let router = secured_app_with(McpAuthConfig {
        public_client_id: Some("mcp-public".into()),
        enforce_azp: Some(true),
        ..offline_auth()
    })
    .await;
    assert_eq!(
        initialize_status(&router, &token_for("mcp-public", &[])).await,
        StatusCode::OK
    );
    assert_eq!(
        initialize_status(&router, &token_for("mcp-saas", &[])).await,
        StatusCode::UNAUTHORIZED
    );
}

#[r2e_core::test]
async fn azp_is_not_enforced_by_default() {
    let router = secured_app_with(multi_auth()).await;
    assert_eq!(
        initialize_status(&router, &token_for("some-other-client", &["mcp"])).await,
        StatusCode::OK
    );
}

// ── Validation: allowed-scopes ─────────────────────────────────────────────

#[r2e_core::test]
async fn allowed_scopes_cap_the_effective_scopes() {
    let router = secured_app_with(enforcing()).await;
    let scopes = &["mcp", "mcp:read", "mcp:write"];

    // The local-CLI client is capped to `mcp` + `mcp:read`: the write tool
    // disappears from the list and is refused on call, whatever the token
    // carries.
    let token = token_for("mcp-local-cli", scopes);
    let session = initialize_auth(&router, "/mcp", &token).await;
    let names = tool_names(&tools_list_auth(&router, "/mcp", &session, &token).await);
    assert!(names.contains(&"read_data".to_string()), "{names:?}");
    assert!(!names.contains(&"write_data".to_string()), "{names:?}");
    let call = tools_call_auth(&router, "/mcp", &session, &token, "write_data", json!({})).await;
    assert_eq!(call["error"]["data"], "forbidden", "{call}");
    let call = tools_call_auth(&router, "/mcp", &session, &token, "read_data", json!({})).await;
    assert!(call.get("error").is_none(), "{call}");

    // The same grant on the uncapped client keeps everything.
    let token = token_for("mcp-saas", scopes);
    let session = initialize_auth(&router, "/mcp", &token).await;
    let names = tool_names(&tools_list_auth(&router, "/mcp", &session, &token).await);
    assert!(names.contains(&"write_data".to_string()), "{names:?}");
    let call = tools_call_auth(&router, "/mcp", &session, &token, "write_data", json!({})).await;
    assert!(call.get("error").is_none(), "{call}");
}

#[r2e_core::test]
async fn required_scopes_see_the_effective_scopes() {
    let router = secured_app_with(McpAuthConfig {
        required_scopes: Some(vec!["mcp:write".into()]),
        ..enforcing()
    })
    .await;
    let scopes = &["mcp:read", "mcp:write"];
    assert_eq!(
        initialize_status(&router, &token_for("mcp-local-cli", scopes)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        initialize_status(&router, &token_for("mcp-saas", scopes)).await,
        StatusCode::OK
    );
}
