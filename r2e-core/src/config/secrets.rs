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

/// How deep a resolved value may itself resolve placeholders. A cycle is
/// caught by name long before; this bounds a custom resolver that keeps
/// producing new references.
const MAX_DEPTH: usize = 32;

/// Resolve `${...}` placeholders in a string value.
///
/// - `${VAR}` / `${env:VAR}` — the environment variable; `${file:/path}` — the
///   trimmed file content.
/// - `${VAR:default}` / `${env:VAR:default}` — everything after the colon is
///   the default, **verbatim**, used when `VAR` is not set (a variable set to
///   the empty string stays empty). `${VAR:-1}` therefore yields `-1` and
///   `${VAR:}` the empty string: there is no shell-style `:-` operator.
///   `file:` references take no default (`:` belongs to the path).
/// - Placeholders nest: a default may hold one (`${A:${B:x}}`), and a resolved
///   value is rendered in turn. A value that refers back to itself is an
///   error naming the chain (`A -> B -> A`) instead of a hang.
///
/// When variables are missing, the `ConfigError::NotFound` names **all** of
/// them (comma-separated), not only the first.
pub fn resolve_placeholders(
    value: &str,
    resolver: &dyn SecretResolver,
) -> Result<String, ConfigError> {
    let mut missing = Missing::default();
    let rendered = resolve_collecting(value, resolver, &mut missing)?;
    missing.into_result()?;
    Ok(rendered)
}

/// The references that could not be resolved, gathered so one error names
/// them all.
#[derive(Default)]
pub(crate) struct Missing(Vec<String>);

impl Missing {
    pub(crate) fn into_result(mut self) -> Result<(), ConfigError> {
        if self.0.is_empty() {
            return Ok(());
        }
        self.0.sort();
        self.0.dedup();
        Err(ConfigError::NotFound(self.0.join(", ")))
    }
}

/// [`resolve_placeholders`], with missing references recorded in `missing`
/// (and rendered empty) instead of failing — the caller reports them once.
pub(crate) fn resolve_collecting(
    value: &str,
    resolver: &dyn SecretResolver,
    missing: &mut Missing,
) -> Result<String, ConfigError> {
    Renderer {
        source: value,
        resolver,
        stack: Vec::new(),
        missing,
    }
    .render(value)
}

struct Renderer<'a> {
    /// The value being resolved, for error messages.
    source: &'a str,
    resolver: &'a dyn SecretResolver,
    /// References currently being rendered, outermost first.
    stack: Vec<String>,
    missing: &'a mut Missing,
}

impl Renderer<'_> {
    fn render(&mut self, text: &str) -> Result<String, ConfigError> {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find("${") {
            out.push_str(&rest[..start]);
            let body = &rest[start + 2..];
            let end = closing_brace(body).ok_or_else(|| {
                ConfigError::Load(format!("Unclosed placeholder in: {}", self.source))
            })?;
            let resolved = self.reference(&body[..end])?;
            out.push_str(&resolved);
            rest = &body[end + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }

    /// Resolve one reference — the text between `${` and its `}`.
    fn reference(&mut self, reference: &str) -> Result<String, ConfigError> {
        // The whole text first: a custom resolver may use colons in its own
        // reference syntax (`vault:secret/path`).
        if let Some(value) = self.lookup(reference)? {
            return self.nested(reference, &value);
        }
        // `file:` takes no default: the colon is part of the path syntax.
        if !reference.starts_with("file:") {
            // For `env:VAR:default`, skip the colon of the `env:` prefix.
            let from = if reference.starts_with("env:") { 4 } else { 0 };
            if let Some(colon) = reference[from..].find(':') {
                let (name, default) = (&reference[..from + colon], &reference[from + colon + 1..]);
                return match self.lookup(name)? {
                    Some(value) => self.nested(name, &value),
                    None => self.render(default),
                };
            }
        }
        self.missing.0.push(reference.trim().to_string());
        Ok(String::new())
    }

    fn lookup(&self, reference: &str) -> Result<Option<String>, ConfigError> {
        match self.resolver.resolve(reference) {
            Ok(value) => Ok(Some(value)),
            Err(ConfigError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Render the placeholders of a resolved value, refusing a reference that
    /// is already being rendered.
    fn nested(&mut self, reference: &str, value: &str) -> Result<String, ConfigError> {
        if !value.contains("${") {
            return Ok(value.to_string());
        }
        let name = reference.trim();
        if self.stack.iter().any(|seen| seen == name) || self.stack.len() >= MAX_DEPTH {
            let mut chain = self.stack.clone();
            chain.push(name.to_string());
            return Err(ConfigError::Load(format!(
                "Placeholder cycle {} in: {}",
                chain.join(" -> "),
                self.source
            )));
        }
        self.stack.push(name.to_string());
        let rendered = self.render(value);
        self.stack.pop();
        rendered
    }
}

/// Index of the `}` closing a placeholder whose body starts at `body[0]`,
/// skipping the placeholders nested inside it.
fn closing_brace(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                depth += 1;
                i += 1;
            }
            b'}' if depth == 0 => return Some(i),
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}
