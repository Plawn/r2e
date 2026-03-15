use r2e_test::TestJwt;

fn decode_exp(token: &str) -> u64 {
    // JWT is header.payload.signature — decode the payload (base64url)
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3, "JWT should have 3 parts");

    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let payload = URL_SAFE_NO_PAD.decode(parts[1]).expect("failed to decode JWT payload");
    let claims: serde_json::Value =
        serde_json::from_slice(&payload).expect("failed to parse JWT payload");
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
