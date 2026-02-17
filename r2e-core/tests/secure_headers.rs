use r2e_core::secure_headers::SecureHeaders;

fn header_names(sh: &SecureHeaders) -> Vec<String> {
    sh.headers().iter().map(|(n, _)| n.to_string()).collect()
}

fn get_header_value(sh: &SecureHeaders, name: &str) -> Option<String> {
    sh.headers()
        .iter()
        .find(|(n, _)| n.as_str() == name)
        .map(|(_, v)| v.to_str().unwrap().to_string())
}

#[test]
fn default_headers_include_basics() {
    let sh = SecureHeaders::default();
    let names = header_names(&sh);
    assert!(names.contains(&"x-content-type-options".to_string()));
    assert!(names.contains(&"x-frame-options".to_string()));
    assert!(names.contains(&"strict-transport-security".to_string()));
    assert!(names.contains(&"x-xss-protection".to_string()));
    assert!(names.contains(&"referrer-policy".to_string()));
}

#[test]
fn builder_custom_csp() {
    let sh = SecureHeaders::builder()
        .content_security_policy("default-src 'self'")
        .build();
    assert_eq!(
        get_header_value(&sh, "content-security-policy"),
        Some("default-src 'self'".to_string())
    );
}

#[test]
fn builder_disable_frame_options() {
    let sh = SecureHeaders::builder().no_frame_options().build();
    assert!(get_header_value(&sh, "x-frame-options").is_none());
}

#[test]
fn builder_custom_hsts_max_age() {
    let sh = SecureHeaders::builder().hsts_max_age(60000).build();
    assert_eq!(
        get_header_value(&sh, "strict-transport-security"),
        Some("max-age=60000; includeSubDomains".to_string())
    );
}
