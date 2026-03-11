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
///
/// Supports default values with `${VAR:default}` syntax.
/// If the resolver cannot find `VAR`, the `default` is used instead.
/// The colon-default is only recognized for plain and `env:` references,
/// not for `file:` references (where `:` is part of the path syntax).
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

        let resolved = match resolve_with_default(reference, resolver) {
            Ok(val) => val,
            Err(e) => return Err(e),
        };

        result = format!("{}{}{}", &result[..start], resolved, &result[start + end + 1..]);
    }
    Ok(result)
}

/// Try to resolve a reference, supporting `VAR:default` and `env:VAR:default` syntax.
fn resolve_with_default(
    reference: &str,
    resolver: &dyn SecretResolver,
) -> Result<String, ConfigError> {
    // file: references don't support default values (colon is part of path)
    if reference.starts_with("file:") {
        return resolver.resolve(reference);
    }

    // Try resolving as-is first
    match resolver.resolve(reference) {
        Ok(val) => return Ok(val),
        Err(ConfigError::NotFound(_)) => {}
        Err(e) => return Err(e),
    }

    // Check for `:default` syntax
    // For `env:VAR:default`, skip the first colon (the `env:` prefix)
    let search_from = if reference.starts_with("env:") { 4 } else { 0 };
    if let Some(colon_pos) = reference[search_from..].find(':') {
        let colon_pos = search_from + colon_pos;
        let var_ref = &reference[..colon_pos];
        let default = &reference[colon_pos + 1..];
        match resolver.resolve(var_ref) {
            Ok(val) => Ok(val),
            Err(ConfigError::NotFound(_)) => Ok(default.to_string()),
            Err(e) => Err(e),
        }
    } else {
        Err(ConfigError::NotFound(reference.trim().to_string()))
    }
}
