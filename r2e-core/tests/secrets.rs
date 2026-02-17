use r2e_core::config::secrets::{resolve_placeholders, DefaultSecretResolver};

#[test]
fn test_env_var_resolution() {
    unsafe { std::env::set_var("TEST_R2E_DB_URL", "postgres://localhost/test") };
    let resolver = DefaultSecretResolver;
    let result = resolve_placeholders("${TEST_R2E_DB_URL}", &resolver).unwrap();
    assert_eq!(result, "postgres://localhost/test");
    unsafe { std::env::remove_var("TEST_R2E_DB_URL") };
}

#[test]
fn test_explicit_env_resolution() {
    unsafe { std::env::set_var("TEST_R2E_HOST", "myhost") };
    let resolver = DefaultSecretResolver;
    let result = resolve_placeholders("${env:TEST_R2E_HOST}", &resolver).unwrap();
    assert_eq!(result, "myhost");
    unsafe { std::env::remove_var("TEST_R2E_HOST") };
}

#[test]
fn test_mixed_resolution() {
    unsafe { std::env::set_var("TEST_R2E_MIX_HOST", "localhost") };
    let resolver = DefaultSecretResolver;
    let result =
        resolve_placeholders("http://${TEST_R2E_MIX_HOST}:8080/api", &resolver).unwrap();
    assert_eq!(result, "http://localhost:8080/api");
    unsafe { std::env::remove_var("TEST_R2E_MIX_HOST") };
}

#[test]
fn test_no_placeholder() {
    let resolver = DefaultSecretResolver;
    let result = resolve_placeholders("plain-value", &resolver).unwrap();
    assert_eq!(result, "plain-value");
}

#[test]
fn test_unclosed_placeholder() {
    let resolver = DefaultSecretResolver;
    let result = resolve_placeholders("${UNCLOSED", &resolver);
    assert!(result.is_err());
}

#[test]
fn test_file_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let secret_file = dir.path().join("secret.txt");
    std::fs::write(&secret_file, "my-secret-value\n").unwrap();

    let resolver = DefaultSecretResolver;
    let ref_str = format!("${{file:{}}}", secret_file.display());
    let result = resolve_placeholders(&ref_str, &resolver).unwrap();
    assert_eq!(result, "my-secret-value");
}
