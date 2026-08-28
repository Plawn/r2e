use r2e_oidc::{ClientRegistry, InMemoryUserStore, OidcError, OidcServer, OidcUser};

#[test]
fn try_build_rejects_insecure_non_localhost_issuer() {
    let err = build_error(OidcServer::new().issuer("http://example.com"));

    assert!(err.to_string().contains("issuer must use https"));
}

#[test]
fn try_build_rejects_invalid_base_path() {
    let err = build_error(OidcServer::new().base_path("auth"));

    assert!(err.to_string().contains("base_path"));
}

#[test]
fn try_build_rejects_zero_token_ttl() {
    let err = build_error(OidcServer::new().token_ttl(0));

    assert!(err.to_string().contains("token TTL"));
}

#[test]
fn in_memory_store_rejects_duplicate_subjects() {
    let users = InMemoryUserStore::new().add_user(
        "alice",
        "password123",
        OidcUser {
            sub: "user-1".into(),
            ..Default::default()
        },
    );

    let err = match users.try_add_user(
        "bob",
        "password456",
        OidcUser {
            sub: "user-1".into(),
            ..Default::default()
        },
    ) {
        Ok(_) => panic!("expected duplicate subject to fail"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("already assigned"));
}

#[test]
fn client_id_cannot_be_both_public_and_confidential() {
    let public_first = ClientRegistry::new()
        .add_public_client("shared", ["http://127.0.0.1/callback"])
        .try_add_client("shared", "secret")
        .err()
        .expect("public client id must not become confidential");
    assert!(public_first.to_string().contains("public client"));

    let confidential_first = ClientRegistry::new()
        .add_client("shared", "secret")
        .try_add_public_client("shared", ["http://127.0.0.1/callback"])
        .err()
        .expect("confidential client id must not become public");
    assert!(confidential_first
        .to_string()
        .contains("confidential client"));
}

fn build_error(server: OidcServer) -> OidcError {
    match server.try_build() {
        Ok(_) => panic!("expected build to fail"),
        Err(err) => err,
    }
}

// ── Redirect URI host validation (RFC 8252 §7.3) ──────────────────────────

#[test]
fn loopback_redirect_uris_accept_ipv6() {
    // `Url::host_str()` yields "[::1]" — matching on it silently rejects every
    // IPv6 loopback client.
    ClientRegistry::new()
        .try_add_public_client("cli", ["http://[::1]:49152/callback"])
        .expect("IPv6 loopback must be a valid http redirect host");
    ClientRegistry::new()
        .try_add_public_client("cli", ["http://127.0.0.1:49152/callback"])
        .expect("IPv4 loopback must be a valid http redirect host");
    ClientRegistry::new()
        .try_add_public_client("cli", ["http://localhost:49152/callback"])
        .expect("localhost must be a valid http redirect host");
}

#[test]
fn non_loopback_http_redirect_uris_are_rejected() {
    for uri in [
        "http://example.com/callback",
        "http://[2001:db8::1]/callback",
        "http://10.0.0.1/callback",
    ] {
        let err = ClientRegistry::new()
            .try_add_public_client("cli", [uri])
            .err()
            .unwrap_or_else(|| panic!("{uri} must not be accepted over plain http"));
        assert!(err.to_string().contains("loopback"));
    }
}

// ── Client scope allowlists ───────────────────────────────────────────────

#[test]
fn with_scopes_requires_a_registered_client() {
    let err = ClientRegistry::new()
        .try_with_scopes(["openid"])
        .err()
        .expect("with_scopes without a client must fail");
    assert!(err.to_string().contains("must follow"));
}

#[test]
fn with_scopes_rejects_invalid_scope_tokens() {
    for scope in ["", "two words", "quote\"inside"] {
        let err = ClientRegistry::new()
            .add_client("svc", "secret")
            .try_with_scopes([scope])
            .err()
            .unwrap_or_else(|| panic!("scope token `{scope}` must be rejected"));
        assert!(err.to_string().contains("invalid scope token"));
    }
}

#[test]
fn try_build_rejects_invalid_password_grant_scopes() {
    let err = build_error(
        OidcServer::new()
            .with_user_store(InMemoryUserStore::new())
            .password_grant_scopes(["two words"]),
    );
    assert!(err.to_string().contains("password grant scope"));
}
