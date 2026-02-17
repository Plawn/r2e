use r2e_security::error::SecurityError;
use r2e_security::extractor::{extract_bearer_token, extract_bearer_token_from_parts};

#[test]
fn valid_bearer_token() {
    let result = extract_bearer_token("Bearer abc123");
    assert_eq!(result.unwrap(), "abc123");
}

#[test]
fn case_insensitive_scheme() {
    let result = extract_bearer_token("bearer abc123");
    assert_eq!(result.unwrap(), "abc123");
}

#[test]
fn case_insensitive_scheme_mixed() {
    let result = extract_bearer_token("BEARER abc123");
    assert_eq!(result.unwrap(), "abc123");
}

#[test]
fn invalid_scheme_basic() {
    let result = extract_bearer_token("Basic abc123");
    assert!(matches!(result, Err(SecurityError::InvalidAuthScheme)));
}

#[test]
fn empty_authorization_header() {
    let result = extract_bearer_token("");
    assert!(matches!(result, Err(SecurityError::InvalidAuthScheme)));
}

#[test]
fn bearer_only_no_token() {
    // "Bearer " splits into ["Bearer", ""] — returns empty string
    let result = extract_bearer_token("Bearer ");
    assert_eq!(result.unwrap(), "");
}

#[test]
fn token_with_dots() {
    let result = extract_bearer_token("Bearer eyJ.eyJ.sig");
    assert_eq!(result.unwrap(), "eyJ.eyJ.sig");
}

#[test]
fn missing_authorization_header() {
    use r2e_core::http::header::HttpRequest;
    let (parts, _) = HttpRequest::builder()
        .uri("/test")
        .body(())
        .unwrap()
        .into_parts();
    let result = extract_bearer_token_from_parts(&parts);
    assert!(matches!(result, Err(SecurityError::MissingAuthHeader)));
}
