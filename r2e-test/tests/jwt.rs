use r2e_test::TestJwt;
use serde_json::Value;

fn decode_payload(token: &str) -> Value {
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3, "JWT should have 3 parts");

    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let payload = URL_SAFE_NO_PAD.decode(parts[1]).expect("failed to decode JWT payload");
    serde_json::from_slice(&payload).expect("failed to parse JWT payload")
}

fn decode_header(token: &str) -> Value {
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3, "JWT should have 3 parts");

    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let header = URL_SAFE_NO_PAD.decode(parts[0]).expect("failed to decode JWT header");
    serde_json::from_slice(&header).expect("failed to parse JWT header")
}

fn decode_exp(token: &str) -> u64 {
    let claims = decode_payload(token);
    claims["exp"].as_u64().expect("exp claim missing or not a number")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn test_expired_token_has_past_exp() {
    let jwt = TestJwt::new();
    let token = jwt.token_builder("user").expired().build();
    let exp = decode_exp(&token);
    let now = now_secs();
    assert!(
        exp < now,
        "expired token exp ({exp}) should be in the past (now={now})"
    );
}

#[test]
fn test_expired_token_has_recent_exp() {
    let jwt = TestJwt::new();
    let token = jwt.token_builder("user").expired().build();
    let exp = decode_exp(&token);
    let now = now_secs();
    // exp should be now - 60, so within the last 120 seconds
    assert!(
        exp > now.saturating_sub(120),
        "expired token exp ({exp}) should be within 120s of now ({now})"
    );
}

#[test]
fn test_expires_in_zero_produces_now_exp() {
    let jwt = TestJwt::new();
    let token = jwt.token_builder("user").expires_in_secs(0).build();
    let exp = decode_exp(&token);
    let now = now_secs();
    // expires_in_secs(0) means exp = now + 0, so exp ≈ now (within 2s tolerance)
    assert!(
        exp >= now.saturating_sub(2) && exp <= now + 2,
        "expires_in_secs(0) should produce exp ≈ now, got exp={exp}, now={now}"
    );
}

// ── Negative testing helpers ──

#[test]
fn test_wrong_issuer_token() {
    let jwt = TestJwt::new();
    let token = jwt.wrong_issuer_token("user-1");
    let claims = decode_payload(&token);
    assert_eq!(claims["iss"].as_str().unwrap(), "wrong-issuer");
}

#[test]
fn test_wrong_audience_token() {
    let jwt = TestJwt::new();
    let token = jwt.wrong_audience_token("user-1");
    let claims = decode_payload(&token);
    assert_eq!(claims["aud"].as_str().unwrap(), "wrong-audience");
}

#[test]
fn test_wrong_algorithm_token() {
    let jwt = TestJwt::new();
    let token = jwt.wrong_algorithm_token("user-1");
    let header = decode_header(&token);
    assert_ne!(header["alg"].as_str().unwrap(), "HS256");
}

#[test]
fn test_malformed_token() {
    let token = TestJwt::malformed_token();
    // A valid JWT has exactly 3 dot-separated parts
    let parts: Vec<&str> = token.split('.').collect();
    assert_ne!(parts.len(), 3, "malformed token should not have 3 parts");
}

#[test]
fn test_without_sub() {
    let jwt = TestJwt::new();
    let token = jwt.token_builder("user-1").without_sub().build();
    let claims = decode_payload(&token);
    assert!(claims.get("sub").is_none(), "sub claim should be absent");
}

#[test]
fn test_without_claim() {
    let jwt = TestJwt::new();
    let token = jwt.token_builder("user-1").without_claim("iss").build();
    let claims = decode_payload(&token);
    assert!(claims.get("iss").is_none(), "iss claim should be absent");
    // Other claims should still be present
    assert!(claims.get("sub").is_some());
    assert!(claims.get("aud").is_some());
}

#[test]
fn test_issuer_override_on_builder() {
    let jwt = TestJwt::new();
    let token = jwt
        .token_builder("user-1")
        .issuer("custom-iss")
        .audience("custom-aud")
        .roles(&["admin"])
        .build();
    let claims = decode_payload(&token);
    assert_eq!(claims["iss"].as_str().unwrap(), "custom-iss");
    assert_eq!(claims["aud"].as_str().unwrap(), "custom-aud");
    assert_eq!(claims["sub"].as_str().unwrap(), "user-1");
}
