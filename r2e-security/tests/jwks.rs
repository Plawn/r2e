use std::time::{Duration, Instant};

use jsonwebtoken::Algorithm;
use r2e_security::jwks::{
    can_attempt, is_stale, kty_for_algorithm, validate_jwks_url, CachedJwk,
};

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

// ── CachedJwk key type tests ──

#[test]
fn rsa_key_requires_n_and_e() {
    let jwk = CachedJwk {
        kty: "RSA".into(),
        alg: None,
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
        n: None,
        e: None,
        x: Some("x".into()),
        y: Some("y".into()),
    };
    let err = format!("{:?}", jwk.to_decoding_key_checked(Algorithm::RS256).unwrap_err());
    assert!(err.contains("incompatible with token algorithm"));
}

#[test]
fn checked_rejects_alg_mismatch() {
    // kty matches the family, but the JWK advertises a different alg.
    let jwk = CachedJwk {
        kty: "RSA".into(),
        alg: Some("RS512".into()),
        n: Some("n".into()),
        e: Some("AQAB".into()),
        x: None,
        y: None,
    };
    let err = format!("{:?}", jwk.to_decoding_key_checked(Algorithm::RS256).unwrap_err());
    assert!(err.contains("does not match token algorithm"));
}
