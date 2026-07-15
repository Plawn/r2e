use std::time::{Duration, Instant};

use jsonwebtoken::Algorithm;
use r2e_security::SecurityConfig;
use r2e_security::jwks::{
    CachedJwk, JwksCache, build_jwks_client, can_attempt, can_use_stale, is_stale,
    kty_for_algorithm, validate_jwks_url,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TEST_JWKS: &str = r#"{"keys":[{"kid":"test-key","kty":"RSA","alg":"RS256","use":"sig","key_ops":["verify"],"n":"AQ","e":"AQAB"}]}"#;

async fn serve_jwks_once() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request).await.unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            TEST_JWKS.len(),
            TEST_JWKS,
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    (format!("http://{address}/jwks.json"), server)
}

// ── staleness / refresh interval ──

#[test]
fn stale_when_never_refreshed() {
    assert!(is_stale(None, Duration::from_secs(60)));
}

#[test]
fn stale_when_ttl_elapsed() {
    let ts = Instant::now() - Duration::from_secs(61);
    assert!(is_stale(Some(ts), Duration::from_secs(60)));
}

#[test]
fn not_stale_before_ttl() {
    let ts = Instant::now() - Duration::from_secs(10);
    assert!(!is_stale(Some(ts), Duration::from_secs(60)));
}

#[test]
fn can_attempt_when_never_attempted() {
    assert!(can_attempt(None, Duration::from_secs(10)));
}

#[test]
fn can_attempt_when_interval_elapsed() {
    let ts = Instant::now() - Duration::from_secs(11);
    assert!(can_attempt(Some(ts), Duration::from_secs(10)));
}

#[test]
fn cannot_attempt_too_soon() {
    let ts = Instant::now() - Duration::from_secs(3);
    assert!(!can_attempt(Some(ts), Duration::from_secs(10)));
}

#[test]
fn stale_key_is_usable_within_grace_period() {
    let ts = Instant::now() - Duration::from_secs(75);
    assert!(can_use_stale(
        Some(ts),
        Duration::from_secs(60),
        Duration::from_secs(30),
    ));
}

#[test]
fn stale_key_is_rejected_after_grace_period() {
    let ts = Instant::now() - Duration::from_secs(91);
    assert!(!can_use_stale(
        Some(ts),
        Duration::from_secs(60),
        Duration::from_secs(30),
    ));
    assert!(!can_use_stale(
        None,
        Duration::from_secs(60),
        Duration::from_secs(30),
    ));
}

#[r2e_core::test]
async fn cache_serves_expired_key_within_grace_period() {
    let (url, server) = serve_jwks_once().await;
    let config = SecurityConfig::new(url, "iss", "aud")
        .allow_insecure_jwks_url()
        .with_cache_ttl(0)
        .with_max_stale(60)
        .with_min_refresh_interval(3600);
    let cache = JwksCache::new(config).await.unwrap();
    server.await.unwrap();

    assert!(cache.get_key("test-key", Algorithm::RS256).await.is_ok());
}

#[r2e_core::test]
async fn cache_rejects_expired_key_after_grace_period() {
    let (url, server) = serve_jwks_once().await;
    let config = SecurityConfig::new(url, "iss", "aud")
        .allow_insecure_jwks_url()
        .with_cache_ttl(0)
        .with_max_stale(0)
        .with_min_refresh_interval(3600);
    let cache = JwksCache::new(config).await.unwrap();
    server.await.unwrap();

    let error = cache
        .get_key("test-key", Algorithm::RS256)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("maximum stale grace period"));
}

// ── CachedJwk key type tests ──

#[test]
fn rsa_key_requires_n_and_e() {
    let jwk = CachedJwk {
        kty: "RSA".into(),
        alg: None,
        key_use: None,
        key_ops: None,
        n: None,
        e: Some("AQAB".into()),
        x: None,
        y: None,
    };
    let err = format!("{:?}", jwk.to_decoding_key().unwrap_err());
    assert!(err.contains("'n' component"));
}

#[test]
fn ec_key_requires_x_and_y() {
    let jwk = CachedJwk {
        kty: "EC".into(),
        alg: None,
        key_use: None,
        key_ops: None,
        n: None,
        e: None,
        x: Some("test".into()),
        y: None,
    };
    let err = format!("{:?}", jwk.to_decoding_key().unwrap_err());
    assert!(err.contains("'y' component"));
}

#[test]
fn okp_key_requires_x() {
    let jwk = CachedJwk {
        kty: "OKP".into(),
        alg: None,
        key_use: None,
        key_ops: None,
        n: None,
        e: None,
        x: None,
        y: None,
    };
    let err = format!("{:?}", jwk.to_decoding_key().unwrap_err());
    assert!(err.contains("'x' component"));
}

#[test]
fn unsupported_key_type_rejected() {
    let jwk = CachedJwk {
        kty: "unknown".into(),
        alg: None,
        key_use: None,
        key_ops: None,
        n: None,
        e: None,
        x: None,
        y: None,
    };
    let err = format!("{:?}", jwk.to_decoding_key().unwrap_err());
    assert!(err.contains("Unsupported key type"));
}

// ── JWKS URL validation ──

#[test]
fn https_url_accepted() {
    assert!(validate_jwks_url("https://auth.example.com/jwks.json", false).is_ok());
}

#[test]
fn https_scheme_is_case_insensitive() {
    assert!(validate_jwks_url("HTTPS://auth.example.com/jwks.json", false).is_ok());
}

#[test]
fn http_url_rejected_by_default() {
    assert!(validate_jwks_url("http://auth.example.com/jwks.json", false).is_err());
}

#[test]
fn http_url_allowed_when_insecure_opt_in() {
    assert!(validate_jwks_url("http://localhost:8080/jwks.json", true).is_ok());
}

#[test]
fn non_http_scheme_rejected_even_with_insecure_opt_in() {
    assert!(validate_jwks_url("file:///tmp/jwks.json", true).is_err());
}

#[r2e_core::test]
async fn strict_client_rejects_http_targets() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = SecurityConfig::new("https://auth.example.com/jwks.json", "iss", "aud");
    let client = build_jwks_client(&config).unwrap();

    let result = client
        .get(format!("http://{address}/jwks.json"))
        .send()
        .await;
    assert!(result.is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err(),
        "strict client must reject HTTP before opening a connection"
    );
}

// ── kty / algorithm compatibility (defense-in-depth) ──

#[test]
fn kty_mapping_covers_families() {
    assert_eq!(kty_for_algorithm(Algorithm::RS256), "RSA");
    assert_eq!(kty_for_algorithm(Algorithm::PS512), "RSA");
    assert_eq!(kty_for_algorithm(Algorithm::ES256), "EC");
    assert_eq!(kty_for_algorithm(Algorithm::EdDSA), "OKP");
    assert_eq!(kty_for_algorithm(Algorithm::HS256), "oct");
}

#[test]
fn checked_rejects_kty_mismatch() {
    // An EC key cannot be used to verify an RS256 token.
    let jwk = CachedJwk {
        kty: "EC".into(),
        alg: None,
        key_use: None,
        key_ops: None,
        n: None,
        e: None,
        x: Some("x".into()),
        y: Some("y".into()),
    };
    let err = format!(
        "{:?}",
        jwk.to_decoding_key_checked(Algorithm::RS256).unwrap_err()
    );
    assert!(err.contains("incompatible with token algorithm"));
}

#[test]
fn checked_rejects_alg_mismatch() {
    // kty matches the family, but the JWK advertises a different alg.
    let jwk = CachedJwk {
        kty: "RSA".into(),
        alg: Some("RS512".into()),
        key_use: None,
        key_ops: None,
        n: Some("n".into()),
        e: Some("AQAB".into()),
        x: None,
        y: None,
    };
    let err = format!(
        "{:?}",
        jwk.to_decoding_key_checked(Algorithm::RS256).unwrap_err()
    );
    assert!(err.contains("does not match token algorithm"));
}

#[test]
fn checked_rejects_encryption_key_use() {
    let jwk = CachedJwk {
        kty: "RSA".into(),
        alg: Some("RS256".into()),
        key_use: Some("enc".into()),
        key_ops: None,
        n: Some("AQ".into()),
        e: Some("AQAB".into()),
        x: None,
        y: None,
    };

    let err = jwk.to_decoding_key_checked(Algorithm::RS256).unwrap_err();
    assert!(
        err.to_string()
            .contains("does not permit signature verification")
    );
}

#[test]
fn checked_rejects_key_ops_without_verify() {
    let jwk = CachedJwk {
        kty: "RSA".into(),
        alg: Some("RS256".into()),
        key_use: Some("sig".into()),
        key_ops: Some(vec!["encrypt".into()]),
        n: Some("AQ".into()),
        e: Some("AQAB".into()),
        x: None,
        y: None,
    };

    let err = jwk.to_decoding_key_checked(Algorithm::RS256).unwrap_err();
    assert!(err.to_string().contains("key_ops"));
}

#[test]
fn checked_accepts_signature_use_and_verify_operation() {
    let jwk = CachedJwk {
        kty: "RSA".into(),
        alg: Some("RS256".into()),
        key_use: Some("sig".into()),
        key_ops: Some(vec!["verify".into()]),
        n: Some("AQ".into()),
        e: Some("AQAB".into()),
        x: None,
        y: None,
    };

    assert!(jwk.to_decoding_key_checked(Algorithm::RS256).is_ok());
}
