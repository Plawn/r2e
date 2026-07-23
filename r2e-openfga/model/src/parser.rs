//! Hand-rolled parser for the OpenFGA DSL (schema 1.1).
//!
//! Statements are line-oriented (`model`, `schema`, `type`, `relations`,
//! `define`, `condition`); relation rewrite expressions are parsed with a
//! small recursive-descent parser over a token stream. Condition bodies are
//! CEL passthrough: the text between the braces is captured verbatim
//! (string-literal-aware brace matching), never interpreted.
//!
//! Matches the official `openfga/language` transformer output (validated
//! against its vendored DSL↔JSON corpus). Like the official transformer,
//! this is *syntax only* — semantic checks (unknown relation, unknown
//! condition, ...) live in [`AuthorizationModel::validate`][crate::validate].

use std::collections::BTreeMap;

use crate::model::{
    AuthorizationModel, Condition, ConditionParamType, Metadata, ObjectRelation, RelationMetadata,
    RelationReference, TypeDefinition, Userset, Wildcard,
};

/// A parse failure, with the 1-based source line it occurred on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parse OpenFGA DSL source into an [`AuthorizationModel`].
///
/// Syntax-level errors (bad structure, mixed operators without parentheses,
/// duplicate names) are reported with their source line. Semantic issues are
/// deliberately not checked here — call
/// [`validate`][crate::validate::validate] for those.
pub fn parse(dsl: &str) -> Result<AuthorizationModel, ParseError> {
    Parser::new(dsl).parse_model()
}

fn err<T>(line: usize, message: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError {
        line,
        message: message.into(),
    })
}

// ---------------------------------------------------------------------------
// Statement-level parser
// ---------------------------------------------------------------------------

struct Parser<'a> {
    /// (1-based line number, raw line) — every source line, untouched.
    /// Statement reads (`peek`/`next`) skip blank/comment lines and strip
    /// trailing comments; condition bodies read raw lines (`next_raw`) so
    /// CEL is captured verbatim (a `#` inside a CEL string is not a comment).
    lines: Vec<(usize, &'a str)>,
    pos: usize,
}

/// Strip a trailing `# comment` from a **statement** line. A `#` only starts
/// a comment when preceded by whitespace (or at line start) — `team#member`
/// stays intact. Never applied to condition bodies (CEL string literals may
/// contain `#`).
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            return &line[..i];
        }
    }
    line
}

/// Blank or comment-only — invisible between statements.
fn is_skippable(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.is_empty() || trimmed.starts_with('#')
}

impl<'a> Parser<'a> {
    fn new(dsl: &'a str) -> Self {
        let lines = dsl.lines().enumerate().map(|(i, l)| (i + 1, l)).collect();
        Parser { lines, pos: 0 }
    }

    /// Next statement line (blank/comment lines skipped, trailing comment
    /// stripped), without advancing.
    fn peek(&self) -> Option<(usize, &'a str)> {
        self.lines[self.pos..]
            .iter()
            .find(|(_, l)| !is_skippable(l))
            .map(|&(n, l)| (n, strip_comment(l)))
    }

    /// Consume and return the next statement line.
    fn next(&mut self) -> Option<(usize, &'a str)> {
        while let Some(&(n, l)) = self.lines.get(self.pos) {
            self.pos += 1;
            if !is_skippable(l) {
                return Some((n, strip_comment(l)));
            }
        }
        None
    }

    /// Consume the next line verbatim (condition bodies).
    fn next_raw(&mut self) -> Option<(usize, &'a str)> {
        let line = self.lines.get(self.pos).copied();
        if line.is_some() {
            self.pos += 1;
        }
        line
    }

    fn last_line_no(&self) -> usize {
        self.lines.last().map(|(n, _)| *n).unwrap_or(1)
    }

    fn parse_model(&mut self) -> Result<AuthorizationModel, ParseError> {
        // Header: `model` then `schema 1.1`.
        let (line_no, line) = match self.next() {
            Some(l) => l,
            None => return err(1, "empty model: expected `model` header"),
        };
        if line.trim() != "model" {
            return err(
                line_no,
                format!("expected `model` header, found `{}`", line.trim()),
            );
        }
        let (line_no, line) = match self.next() {
            Some(l) => l,
            None => return err(line_no, "expected `schema <version>` after `model`"),
        };
        let schema_version = match line.trim().strip_prefix("schema") {
            Some(v) if !v.is_empty() && v.starts_with(char::is_whitespace) => v.trim().to_string(),
            _ => {
                return err(
                    line_no,
                    format!("expected `schema <version>`, found `{}`", line.trim()),
                )
            }
        };
        if schema_version != "1.1" {
            return err(
                line_no,
                format!(
                    "unsupported schema version `{}`: only schema 1.1 is supported \
                     (modular models / `module` are not)",
                    schema_version
                ),
            );
        }

        let mut model = AuthorizationModel {
            schema_version,
            type_definitions: Vec::new(),
            conditions: BTreeMap::new(),
        };

        while let Some((line_no, line)) = self.next() {
            let trimmed = line.trim();
            if let Some(rest) = keyword_rest(trimmed, "type") {
                let type_def = self.parse_type(line_no, rest)?;
                if model
                    .type_definitions
                    .iter()
                    .any(|t| t.type_name == type_def.type_name)
                {
                    return err(line_no, format!("duplicate type `{}`", type_def.type_name));
                }
                model.type_definitions.push(type_def);
            } else if keyword_rest(trimmed, "condition").is_some() {
                // Re-read the header verbatim: the body may start on this
                // line, and CEL must not go through comment stripping.
                let raw = self.lines[self.pos - 1].1.trim();
                let cond = self.parse_condition(line_no, raw)?;
                if model.conditions.contains_key(&cond.name) {
                    return err(line_no, format!("duplicate condition `{}`", cond.name));
                }
                model.conditions.insert(cond.name.clone(), cond);
            } else if keyword_rest(trimmed, "module").is_some()
                || keyword_rest(trimmed, "extend").is_some()
            {
                return err(
                    line_no,
                    format!(
                        "`{}` is not supported: modular models (schema 1.2) are out of scope",
                        trimmed.split_whitespace().next().unwrap_or(trimmed)
                    ),
                );
            } else {
                return err(
                    line_no,
                    format!(
                        "expected `type <name>` or `condition <name>(...)`, found `{}`",
                        trimmed
                    ),
                );
            }
        }

        Ok(model)
    }

    fn parse_type(&mut self, line_no: usize, rest: &str) -> Result<TypeDefinition, ParseError> {
        let type_name = rest.trim();
        if !is_identifier(type_name) {
            return err(line_no, format!("invalid type name `{}`", type_name));
        }

        let mut type_def = TypeDefinition {
            type_name: type_name.to_string(),
            relations: BTreeMap::new(),
            metadata: None,
        };

        // Optional `relations` block.
        let Some((_, next_line)) = self.peek() else {
            return Ok(type_def);
        };
        if next_line.trim() != "relations" {
            return Ok(type_def);
        }
        let (relations_line, _) = self.next().unwrap();

        let mut metadata = Metadata {
            relations: BTreeMap::new(),
        };
        let mut saw_define = false;

        while let Some((line_no, line)) = self.peek() {
            let trimmed = line.trim();
            let Some(rest) = keyword_rest(trimmed, "define") else {
                break;
            };
            self.next();
            saw_define = true;

            let (name, direct_types, rewrite) = parse_define(line_no, rest)?;
            if type_def.relations.contains_key(&name) {
                return err(
                    line_no,
                    format!(
                        "duplicate relation `{}` on type `{}`",
                        name, type_def.type_name
                    ),
                );
            }
            type_def.relations.insert(name.clone(), rewrite);
            metadata.relations.insert(
                name,
                RelationMetadata {
                    directly_related_user_types: direct_types,
                },
            );
        }

        if !saw_define {
            return err(relations_line, "`relations` block without any `define`");
        }
        type_def.metadata = Some(metadata);
        Ok(type_def)
    }

    /// Parse a `condition name(p: type, ...) { <cel> }` block. The body may
    /// span multiple lines; braces inside CEL string literals are ignored.
    fn parse_condition(
        &mut self,
        line_no: usize,
        first_line: &str,
    ) -> Result<Condition, ParseError> {
        let rest = keyword_rest(first_line, "condition").unwrap();

        let open_paren = match rest.find('(') {
            Some(i) => i,
            None => return err(line_no, "expected `(` after condition name"),
        };
        let name = rest[..open_paren].trim();
        if !is_identifier(name) {
            return err(line_no, format!("invalid condition name `{}`", name));
        }
        let after = &rest[open_paren + 1..];
        let close_paren = match after.find(')') {
            Some(i) => i,
            None => return err(line_no, "expected `)` closing the condition parameter list"),
        };
        let parameters = parse_condition_params(line_no, &after[..close_paren])?;

        let after_params = after[close_paren + 1..].trim_start();
        let Some(body_start) = after_params.strip_prefix('{') else {
            return err(line_no, "expected `{` opening the condition body");
        };

        // Capture the raw CEL between the braces, across lines if needed.
        let mut body = String::new();
        let mut depth = 1usize;
        let mut segment = body_start;
        loop {
            match scan_braces(segment, depth) {
                ScanOutcome::Closed(end) => {
                    body.push_str(&segment[..end]);
                    // After the closing brace we are outside CEL again — a
                    // trailing `# comment` is allowed, anything else is not.
                    let trailing = strip_comment(&segment[end + 1..]).trim();
                    if !trailing.is_empty() {
                        return err(
                            line_no,
                            format!("unexpected `{}` after condition body", trailing),
                        );
                    }
                    break;
                }
                ScanOutcome::Open(new_depth) => {
                    depth = new_depth;
                    body.push_str(segment);
                    body.push('\n');
                    let Some((_, next_line)) = self.next_raw() else {
                        return err(
                            self.last_line_no(),
                            format!("unterminated condition body for `{}`", name),
                        );
                    };
                    segment = next_line;
                }
            }
        }

        Ok(Condition {
            name: name.to_string(),
            expression: body.trim().to_string(),
            parameters,
        })
    }
}

/// `keyword_rest("type user", "type") == Some(" user")`; `None` when the
/// line does not start with the keyword as a full word.
fn keyword_rest<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(keyword)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) || rest.starts_with('(') {
        Some(rest)
    } else {
        None
    }
}

fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

enum ScanOutcome {
    /// Byte offset of the closing `}` within the segment.
    Closed(usize),
    /// Segment ended with this brace depth still open.
    Open(usize),
}

/// Scan one line of CEL for the closing brace, skipping string literals
/// (single- and double-quoted, with backslash escapes).
fn scan_braces(segment: &str, mut depth: usize) -> ScanOutcome {
    let bytes = segment.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return ScanOutcome::Closed(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    ScanOutcome::Open(depth)
}

/// Parse `p1: string, p2: list<string>` into the JSON parameter map.
fn parse_condition_params(
    line_no: usize,
    params: &str,
) -> Result<BTreeMap<String, ConditionParamType>, ParseError> {
    let mut map = BTreeMap::new();
    let params = params.trim();
    if params.is_empty() {
        return Ok(map);
    }
    for part in params.split(',') {
        let (name, ty) = match part.split_once(':') {
            Some(p) => p,
            None => {
                return err(
                    line_no,
                    format!(
                        "expected `name: type` in condition parameters, found `{}`",
                        part.trim()
                    ),
                )
            }
        };
        let name = name.trim();
        if !is_identifier(name) {
            return err(
                line_no,
                format!("invalid condition parameter name `{}`", name),
            );
        }
        let ty = parse_param_type(line_no, ty.trim())?;
        if map.insert(name.to_string(), ty).is_some() {
            return err(line_no, format!("duplicate condition parameter `{}`", name));
        }
    }
    Ok(map)
}

fn parse_param_type(line_no: usize, ty: &str) -> Result<ConditionParamType, ParseError> {
    if let Some(inner) = ty.strip_prefix("list<").and_then(|s| s.strip_suffix('>')) {
        return Ok(ConditionParamType {
            type_name: "TYPE_NAME_LIST".to_string(),
            generic_types: vec![parse_param_type(line_no, inner.trim())?],
        });
    }
    if let Some(inner) = ty.strip_prefix("map<").and_then(|s| s.strip_suffix('>')) {
        return Ok(ConditionParamType {
            type_name: "TYPE_NAME_MAP".to_string(),
            generic_types: vec![parse_param_type(line_no, inner.trim())?],
        });
    }
    let type_name = match ty {
        "bool" => "TYPE_NAME_BOOL",
        "string" => "TYPE_NAME_STRING",
        "int" => "TYPE_NAME_INT",
        "uint" => "TYPE_NAME_UINT",
        "double" => "TYPE_NAME_DOUBLE",
        "duration" => "TYPE_NAME_DURATION",
        "timestamp" => "TYPE_NAME_TIMESTAMP",
        "ipaddress" => "TYPE_NAME_IPADDRESS",
        _ => {
            return err(
                line_no,
                format!("unknown condition parameter type `{}`", ty),
            )
        }
    };
    Ok(ConditionParamType {
        type_name: type_name.to_string(),
        generic_types: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Relation rewrite expressions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token<'a> {
    Ident(&'a str),
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Star,
    Hash,
}

impl std::fmt::Display for Token<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Token::Ident(s) => write!(f, "`{}`", s),
            Token::LParen => write!(f, "`(`"),
            Token::RParen => write!(f, "`)`"),
            Token::LBracket => write!(f, "`[`"),
            Token::RBracket => write!(f, "`]`"),
            Token::Comma => write!(f, "`,`"),
            Token::Colon => write!(f, "`:`"),
            Token::Star => write!(f, "`*`"),
            Token::Hash => write!(f, "`#`"),
        }
    }
}

fn tokenize(line_no: usize, input: &str) -> Result<Vec<Token<'_>>, ParseError> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b' ' | b'\t' => i += 1,
            b'(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            b')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            b'[' => {
                tokens.push(Token::LBracket);
                i += 1;
            }
            b']' => {
                tokens.push(Token::RBracket);
                i += 1;
            }
            b',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            b':' => {
                tokens.push(Token::Colon);
                i += 1;
            }
            b'*' => {
                tokens.push(Token::Star);
                i += 1;
            }
            b'#' => {
                tokens.push(Token::Hash);
                i += 1;
            }
            _ if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-')
                {
                    i += 1;
                }
                tokens.push(Token::Ident(&input[start..i]));
            }
            _ => {
                return err(
                    line_no,
                    format!(
                        "unexpected character `{}`",
                        &input[i..].chars().next().unwrap()
                    ),
                );
            }
        }
    }
    Ok(tokens)
}

/// Parse `define <name>: <expr>` (the part after `define`). Returns the
/// relation name, its direct type restrictions (empty when no `[...]`),
/// and the rewrite tree.
fn parse_define(
    line_no: usize,
    rest: &str,
) -> Result<(String, Vec<RelationReference>, Userset), ParseError> {
    let tokens = tokenize(line_no, rest)?;
    let mut p = ExprParser {
        line_no,
        tokens: &tokens,
        pos: 0,
        direct_types: None,
    };

    let name = match p.next_token() {
        Some(Token::Ident(name)) => name.to_string(),
        other => return p.unexpected(other, "relation name"),
    };
    match p.next_token() {
        Some(Token::Colon) => {}
        other => return p.unexpected(other, "`:` after relation name"),
    }

    let rewrite = p.parse_expr()?;
    if let Some(tok) = p.next_token() {
        return err(
            line_no,
            format!("unexpected {} after relation expression", tok),
        );
    }

    Ok((name, p.direct_types.take().unwrap_or_default(), rewrite))
}

struct ExprParser<'a, 't> {
    line_no: usize,
    tokens: &'a [Token<'t>],
    pos: usize,
    /// Filled when the (single) `[...]` block is parsed.
    direct_types: Option<Vec<RelationReference>>,
}

impl<'a, 't> ExprParser<'a, 't> {
    fn peek_token(&self) -> Option<&Token<'t>> {
        self.tokens.get(self.pos)
    }

    fn next_token(&mut self) -> Option<Token<'t>> {
        let tok = self.tokens.get(self.pos).cloned();
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn unexpected<T>(&self, found: Option<Token<'t>>, expected: &str) -> Result<T, ParseError> {
        match found {
            Some(tok) => err(
                self.line_no,
                format!("expected {}, found {}", expected, tok),
            ),
            None => err(
                self.line_no,
                format!("expected {}, found end of line", expected),
            ),
        }
    }

    /// One level of `or` / `and` / `but not`. Operators cannot be mixed at
    /// the same level — parentheses are required (matches the official DSL).
    fn parse_expr(&mut self) -> Result<Userset, ParseError> {
        let first = self.parse_term()?;
        match self.peek_token() {
            Some(Token::Ident("or")) => {
                let mut children = vec![first];
                while matches!(self.peek_token(), Some(Token::Ident("or"))) {
                    self.pos += 1;
                    children.push(self.parse_term()?);
                }
                self.reject_operator_mix("or")?;
                Ok(Userset::Union { child: children })
            }
            Some(Token::Ident("and")) => {
                let mut children = vec![first];
                while matches!(self.peek_token(), Some(Token::Ident("and"))) {
                    self.pos += 1;
                    children.push(self.parse_term()?);
                }
                self.reject_operator_mix("and")?;
                Ok(Userset::Intersection { child: children })
            }
            Some(Token::Ident("but")) => {
                self.pos += 1;
                match self.next_token() {
                    Some(Token::Ident("not")) => {}
                    other => return self.unexpected(other, "`not` after `but`"),
                }
                let subtract = self.parse_term()?;
                self.reject_operator_mix("but not")?;
                Ok(Userset::Difference {
                    base: Box::new(first),
                    subtract: Box::new(subtract),
                })
            }
            _ => Ok(first),
        }
    }

    /// After a completed operator chain, the next token must close the
    /// expression (`)` / end of line) — a different operator here means
    /// ambiguous mixing.
    fn reject_operator_mix(&self, current: &str) -> Result<(), ParseError> {
        if let Some(Token::Ident(op @ ("or" | "and" | "but"))) = self.peek_token() {
            let other = if *op == "but" { "but not" } else { op };
            return err(
                self.line_no,
                format!(
                    "cannot mix `{}` with `{}` without parentheses",
                    current, other
                ),
            );
        }
        Ok(())
    }

    fn parse_term(&mut self) -> Result<Userset, ParseError> {
        match self.next_token() {
            Some(Token::LParen) => {
                let expr = self.parse_expr()?;
                match self.next_token() {
                    Some(Token::RParen) => Ok(expr),
                    other => self.unexpected(other, "`)`"),
                }
            }
            Some(Token::LBracket) => {
                if self.direct_types.is_some() {
                    return err(
                        self.line_no,
                        "only one `[...]` direct type restriction is allowed per relation",
                    );
                }
                let refs = self.parse_direct_types()?;
                self.direct_types = Some(refs);
                Ok(Userset::This {})
            }
            Some(Token::Ident(name)) if !is_reserved(name) => {
                if matches!(self.peek_token(), Some(Token::Ident("from"))) {
                    self.pos += 1;
                    let tupleset = match self.next_token() {
                        Some(Token::Ident(ts)) if !is_reserved(ts) => ts.to_string(),
                        other => return self.unexpected(other, "relation name after `from`"),
                    };
                    Ok(Userset::TupleToUserset {
                        tupleset: ObjectRelation { relation: tupleset },
                        computed_userset: ObjectRelation {
                            relation: name.to_string(),
                        },
                    })
                } else {
                    Ok(Userset::ComputedUserset {
                        relation: name.to_string(),
                    })
                }
            }
            other => self.unexpected(other, "a relation, `[...]`, or `(`"),
        }
    }

    /// Parse the inside of `[...]`: `type`, `type:*`, `type#relation`,
    /// each optionally followed by `with <condition>`.
    fn parse_direct_types(&mut self) -> Result<Vec<RelationReference>, ParseError> {
        let mut refs = Vec::new();
        loop {
            let type_name = match self.next_token() {
                Some(Token::Ident(t)) => t.to_string(),
                other => return self.unexpected(other, "a type name in `[...]`"),
            };
            let mut reference = RelationReference::direct(type_name);

            match self.peek_token() {
                Some(Token::Colon) => {
                    self.pos += 1;
                    match self.next_token() {
                        Some(Token::Star) => reference.wildcard = Some(Wildcard {}),
                        other => return self.unexpected(other, "`*` after `:` (wildcard)"),
                    }
                }
                Some(Token::Hash) => {
                    self.pos += 1;
                    match self.next_token() {
                        Some(Token::Ident(rel)) => reference.relation = Some(rel.to_string()),
                        other => return self.unexpected(other, "a relation name after `#`"),
                    }
                }
                _ => {}
            }

            if matches!(self.peek_token(), Some(Token::Ident("with"))) {
                self.pos += 1;
                match self.next_token() {
                    Some(Token::Ident(cond)) => reference.condition = Some(cond.to_string()),
                    other => return self.unexpected(other, "a condition name after `with`"),
                }
            }

            refs.push(reference);
            match self.next_token() {
                Some(Token::Comma) => continue,
                Some(Token::RBracket) => break,
                other => return self.unexpected(other, "`,` or `]`"),
            }
        }
        Ok(refs)
    }
}

fn is_reserved(ident: &str) -> bool {
    matches!(ident, "or" | "and" | "but" | "not" | "from" | "with")
}
