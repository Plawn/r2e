//! Small reverse matcher for the RFC 6570 forms used by MCP resource templates.
//!
//! Matching mirrors expansion: a simple `{var}` (level 1) only expands
//! unreserved characters (everything else is percent-encoded), so its reverse
//! match stops at the first reserved character — `r2e://users/{id}` does NOT
//! swallow `1/posts/2`. Only `{+var}` / `{#var}` accept reserved characters.
//! Form-style expressions (`{?q}`, `{&q}`, `{;q}`) and the `.`/`/` prefix
//! forms expand to nothing when their variables are undefined, so they are
//! optional on the way back in. Captured values are percent-decoded.

use std::collections::BTreeMap;

use percent_encoding::percent_decode_str;

#[derive(Debug)]
pub(crate) struct UriTemplate {
    raw: String,
    tokens: Vec<Token>,
}

#[derive(Debug)]
enum Token {
    Literal(String),
    Expression(Expression),
}

#[derive(Debug)]
struct Expression {
    operator: Option<char>,
    variables: Vec<String>,
}

impl UriTemplate {
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        let mut tokens = Vec::new();
        let mut rest = raw;
        while let Some(open) = rest.find('{') {
            let (literal, expression_and_tail) = rest.split_at(open);
            if !literal.is_empty() {
                tokens.push(Token::Literal(literal.to_string()));
            }
            let close = expression_and_tail
                .find('}')
                .ok_or_else(|| "unclosed `{`".to_string())?;
            let body = &expression_and_tail[1..close];
            if body.is_empty() || body.contains('{') {
                return Err("empty or nested expression".to_string());
            }
            let operator = body.chars().next().filter(|c| "+#./;?&".contains(*c));
            let variables = body[operator.map_or(0, char::len_utf8)..]
                .split(',')
                .map(parse_variable)
                .collect::<Result<Vec<_>, _>>()?;
            tokens.push(Token::Expression(Expression {
                operator,
                variables,
            }));
            rest = &expression_and_tail[close + 1..];
        }
        if rest.contains('}') {
            return Err("unmatched `}`".to_string());
        }
        if !rest.is_empty() {
            tokens.push(Token::Literal(rest.to_string()));
        }
        if !tokens.iter().any(|t| matches!(t, Token::Expression(_))) {
            return Err("a template must contain at least one `{variable}`".to_string());
        }
        Ok(Self {
            raw: raw.to_string(),
            tokens,
        })
    }

    pub(crate) fn raw(&self) -> &str {
        &self.raw
    }

    /// The template with variable names erased (`r2e://users/{id}` and
    /// `r2e://users/{uid}` both give `r2e://users/{}`): two templates with
    /// the same shape match exactly the same URIs, so registering both is a
    /// duplicate even though the raw strings differ.
    pub(crate) fn shape(&self) -> String {
        let mut shape = String::with_capacity(self.raw.len());
        for token in &self.tokens {
            match token {
                Token::Literal(literal) => shape.push_str(literal),
                Token::Expression(expression) => {
                    shape.push('{');
                    if let Some(operator) = expression.operator {
                        shape.push(operator);
                    }
                    // Positional forms are shaped by their arity; named forms
                    // by their (sorted) names, which are part of the wire.
                    match expression.operator {
                        Some(';' | '?' | '&') => {
                            let mut names = expression.variables.clone();
                            names.sort();
                            shape.push_str(&names.join(","));
                        }
                        _ => shape.push_str(&expression.variables.len().to_string()),
                    }
                    shape.push('}');
                }
            }
        }
        shape
    }

    pub(crate) fn captures(&self, uri: &str) -> Option<BTreeMap<String, String>> {
        let mut cursor = uri;
        let mut captures = BTreeMap::new();
        for (index, token) in self.tokens.iter().enumerate() {
            match token {
                Token::Literal(literal) => {
                    cursor = cursor.strip_prefix(literal)?;
                }
                Token::Expression(expression) => {
                    // The expansion can only contain characters the operator
                    // would have emitted unencoded; the next token must start
                    // inside that window.
                    let window = expression.window(cursor);
                    let end = match self.tokens.get(index + 1) {
                        Some(Token::Literal(literal)) => {
                            let end = cursor.find(literal.as_str())?;
                            (end <= window).then_some(end)?
                        }
                        // A following expression starts at its operator
                        // prefix when present; an optional one may be absent
                        // altogether, in which case this expansion runs to
                        // the end of its window.
                        Some(Token::Expression(next)) => match next.prefix() {
                            Some(prefix) => match cursor.find(prefix) {
                                Some(end) if end <= window => end,
                                Some(_) => return None,
                                None => window,
                            },
                            None => window,
                        },
                        None => window,
                    };
                    let (expanded, tail) = cursor.split_at(end);
                    expression.capture(expanded, &mut captures)?;
                    cursor = tail;
                }
            }
        }
        cursor.is_empty().then_some(captures)
    }
}

fn parse_variable(raw: &str) -> Result<String, String> {
    let raw = raw.strip_suffix('*').unwrap_or(raw);
    let name = raw.split_once(':').map_or(raw, |(name, _)| name);
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.'))
    {
        return Err(format!("invalid variable `{raw}`"));
    }
    Ok(name.to_string())
}

/// RFC 3986 `reserved` = gen-delims + sub-delims: the characters a level-1
/// expansion percent-encodes, hence cannot appear inside a captured value.
fn is_reserved(c: char) -> bool {
    matches!(
        c,
        ':' | '/' | '?' | '#' | '[' | ']' | '@' | '!' | '$' | '&' | '\'' | '(' | ')' | '*' | '+'
            | ',' | ';' | '='
    )
}

fn decode(value: &str) -> Option<String> {
    percent_decode_str(value)
        .decode_utf8()
        .ok()
        .map(|decoded| decoded.into_owned())
}

impl Expression {
    fn prefix(&self) -> Option<&'static str> {
        match self.operator {
            Some('#') => Some("#"),
            Some('.') => Some("."),
            Some('/') => Some("/"),
            Some(';') => Some(";"),
            Some('?') => Some("?"),
            Some('&') => Some("&"),
            _ => None,
        }
    }

    /// Length of the longest prefix of `cursor` this expression could have
    /// expanded to.
    fn window(&self, cursor: &str) -> usize {
        match self.operator {
            // Reserved expansion: anything goes.
            Some('+' | '#') => cursor.len(),
            // Form-style / path-style lists: the values are level-1 encoded,
            // but the operator's own separators are emitted verbatim.
            Some(operator @ ('?' | '&' | ';' | '.' | '/')) => {
                let separator = match operator {
                    '?' | '&' => '&',
                    other => other,
                };
                let allowed = |c: char| {
                    c == separator || c == operator || c == '=' || c == '%' || !is_reserved(c)
                };
                cursor.find(|c: char| !allowed(c)).unwrap_or(cursor.len())
            }
            // Simple `{var}` (and `{a,b}`): unreserved + pct-encoded + `,`.
            _ => cursor
                .find(|c: char| c != ',' && c != '%' && is_reserved(c))
                .unwrap_or(cursor.len()),
        }
    }

    fn capture(&self, expanded: &str, captures: &mut BTreeMap<String, String>) -> Option<()> {
        match self.operator {
            None | Some('+') => self.capture_positional(expanded, ',', captures),
            Some('#') => self.capture_positional(expanded.strip_prefix('#')?, ',', captures),
            Some('.') => self.capture_optional_positional(expanded, '.', captures),
            Some('/') => self.capture_optional_positional(expanded, '/', captures),
            Some(';') => self.capture_named(expanded.strip_prefix(';'), ';', captures),
            Some('?') => self.capture_named(expanded.strip_prefix('?'), '&', captures),
            Some('&') => self.capture_named(expanded.strip_prefix('&'), '&', captures),
            _ => None,
        }
    }

    fn capture_positional(
        &self,
        expanded: &str,
        separator: char,
        captures: &mut BTreeMap<String, String>,
    ) -> Option<()> {
        if expanded.is_empty() {
            return None;
        }
        if self.variables.len() == 1 {
            captures.insert(self.variables[0].clone(), decode(expanded)?);
            return Some(());
        }
        let values: Vec<_> = expanded.split(separator).collect();
        (values.len() == self.variables.len()).then_some(())?;
        for (name, value) in self.variables.iter().zip(values) {
            captures.insert(name.clone(), decode(value)?);
        }
        Some(())
    }

    /// `{.x}` / `{/x}`: every variable expands with its own leading
    /// separator; undefined variables expand to nothing.
    fn capture_optional_positional(
        &self,
        expanded: &str,
        separator: char,
        captures: &mut BTreeMap<String, String>,
    ) -> Option<()> {
        let values: Vec<_> = match expanded.strip_prefix(separator) {
            Some(rest) => rest.split(separator).collect(),
            None if expanded.is_empty() => Vec::new(),
            None => return None,
        };
        (values.len() <= self.variables.len()).then_some(())?;
        for (index, name) in self.variables.iter().enumerate() {
            let value = values.get(index).map_or(Some(String::new()), |v| decode(v))?;
            captures.insert(name.clone(), value);
        }
        Some(())
    }

    /// `{;x}` / `{?x}` / `{&x}`: `name=value` pairs; an undefined variable
    /// expands to nothing, so absent names capture as empty strings. Pairs
    /// the template does not declare are foreign to it → no match.
    fn capture_named(
        &self,
        expanded: Option<&str>,
        separator: char,
        captures: &mut BTreeMap<String, String>,
    ) -> Option<()> {
        let mut found = BTreeMap::new();
        if let Some(expanded) = expanded {
            for pair in expanded.split(separator).filter(|pair| !pair.is_empty()) {
                let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
                if !self.variables.iter().any(|v| v == name) {
                    return None;
                }
                found.insert(name, value);
            }
        }
        for name in &self.variables {
            let value = found.get(name.as_str()).map_or(Some(String::new()), |v| decode(v))?;
            captures.insert(name.clone(), value);
        }
        Some(())
    }
}
