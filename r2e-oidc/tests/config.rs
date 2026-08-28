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
