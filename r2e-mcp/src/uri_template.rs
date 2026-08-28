//! Small reverse matcher for the RFC 6570 forms used by MCP resource templates.

use std::collections::BTreeMap;

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

    pub(crate) fn captures(&self, uri: &str) -> Option<BTreeMap<String, String>> {
        let mut cursor = uri;
        let mut captures = BTreeMap::new();
        for (index, token) in self.tokens.iter().enumerate() {
            match token {
                Token::Literal(literal) => {
                    cursor = cursor.strip_prefix(literal)?;
                }
                Token::Expression(expression) => {
                    let next_boundary = self.tokens.get(index + 1).and_then(|token| match token {
                        Token::Literal(literal) => Some(literal.as_str()),
                        Token::Expression(next) => next.prefix(),
                    });
                    let (expanded, tail) = match next_boundary {
                        Some(boundary) => {
                            let end = cursor.find(boundary)?;
                            cursor.split_at(end)
                        }
                        None => (cursor, ""),
                    };
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

    fn capture(&self, expanded: &str, captures: &mut BTreeMap<String, String>) -> Option<()> {
        match self.operator {
            None | Some('+') => self.capture_positional(expanded, ',', captures),
            Some('#') => self.capture_positional(expanded.strip_prefix('#')?, ',', captures),
            Some('.') => self.capture_positional(expanded.strip_prefix('.')?, '.', captures),
            Some('/') => self.capture_positional(expanded.strip_prefix('/')?, '/', captures),
            Some(';') => self.capture_named(expanded, ';', captures),
            Some('?') => self.capture_named(expanded.strip_prefix('?')?, '&', captures),
            Some('&') => self.capture_named(expanded.strip_prefix('&')?, '&', captures),
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
            captures.insert(self.variables[0].clone(), expanded.to_string());
            return Some(());
        }
        let values: Vec<_> = expanded.split(separator).collect();
        (values.len() == self.variables.len()).then_some(())?;
        for (name, value) in self.variables.iter().zip(values) {
            captures.insert(name.clone(), value.to_string());
        }
        Some(())
    }

    fn capture_named(
        &self,
        expanded: &str,
        separator: char,
        captures: &mut BTreeMap<String, String>,
    ) -> Option<()> {
        let mut found = BTreeMap::new();
        for pair in expanded.trim_start_matches(separator).split(separator) {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            found.insert(name, value);
        }
        for name in &self.variables {
            captures.insert(name.clone(), (*found.get(name.as_str())?).to_string());
        }
        Some(())
    }
}
