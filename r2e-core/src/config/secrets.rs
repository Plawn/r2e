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
