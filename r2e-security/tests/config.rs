use r2e_security::config::SecurityConfig;

#[test]
fn config_new_required_fields() {
    let config = SecurityConfig::new(
        "https://auth.example.com/.well-known/jwks.json",
        "my-issuer",
        "my-audience",
    );
    assert_eq!(config.jwks_url, "https://auth.example.com/.well-known/jwks.json");
    assert_eq!(config.issuer, "my-issuer");
    assert_eq!(config.audience, "my-audience");
    assert_eq!(config.jwks_cache_ttl_secs, 3600); // default
}

#[test]
fn config_with_cache_ttl() {
    let config = SecurityConfig::new("url", "iss", "aud").with_cache_ttl(300);
    assert_eq!(config.jwks_cache_ttl_secs, 300);
}

#[test]
fn config_fields_accessible() {
    let config = SecurityConfig::new("u", "i", "a").with_cache_ttl(60);
    // All fields are pub, verify they're accessible and cloneable
    let cloned = config.clone();
    assert_eq!(cloned.jwks_url, "u");
    assert_eq!(cloned.issuer, "i");
    assert_eq!(cloned.audience, "a");
    assert_eq!(cloned.jwks_cache_ttl_secs, 60);
}
