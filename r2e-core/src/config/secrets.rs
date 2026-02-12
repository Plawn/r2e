use super::ConfigError;

/// Trait for secret resolution backends.
pub trait SecretResolver: Send + Sync {
    fn resolve(&self, reference: &str) -> Result<String, ConfigError>;
}

/// Default resolver: env vars and file references.
///
/// Supports the following reference formats:
/// - `${VAR_NAME}` — resolves from environment variable
/// - `${env:VAR_NAME}` — explicit env var resolution
/// - `${file:/path/to/secret}` — reads from file (trimmed)
pub struct DefaultSecretResolver;

impl SecretResolver for DefaultSecretResolver {
    fn resolve(&self, reference: &str) -> Result<String, ConfigError> {
        if let Some(path) = reference.strip_prefix("file:") {
            std::fs::read_to_string(path.trim())
                .map(|s| s.trim().to_string())
                .map_err(|e| ConfigError::Load(format!("Secret file '{}': {}", path.trim(), e)))
        } else if let Some(var) = reference.strip_prefix("env:") {
            std::env::var(var.trim())
                .map_err(|_| ConfigError::NotFound(format!("env:{}", var.trim())))
        } else {
            // Default: env var
            std::env::var(reference.trim())
                .map_err(|_| ConfigError::NotFound(reference.trim().to_string()))
        }
    }
}

/// Resolve `${...}` placeholders in a string value.
pub fn resolve_placeholders(
    value: &str,
    resolver: &dyn SecretResolver,
) -> Result<String, ConfigError> {
    let mut result = value.to_string();
    // Find "${" then everything until "}"
    while let Some(start) = result.find("${") {
        let end = result[start..]
            .find('}')
            .ok_or_else(|| ConfigError::Load(format!("Unclosed placeholder in: {}", value)))?;
        let reference = &result[start + 2..start + end];
        let resolved = resolver.resolve(reference)?;
        result = format!("{}{}{}", &result[..start], resolved, &result[start + end + 1..]);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
