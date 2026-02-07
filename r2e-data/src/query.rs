/// A fluent query builder for constructing SELECT queries.
///
/// # Example
///
/// ```ignore
/// let q = QueryBuilder::new("users")
///     .where_eq("email", "a@b.com")
///     .where_like("name", "%alice%")
///     .order_by("id", true)
///     .limit(10);
/// let (sql, params) = q.build_select("*");
/// ```
#[derive(Debug, Clone, Copy)]
pub enum Dialect {
    /// Generic SQL using `?` placeholders (default).
    Generic,
    /// SQLite-style `?` placeholders.
    Sqlite,
    /// MySQL-style `?` placeholders with backtick quoting.
    MySql,
    /// Postgres-style `$1, $2, ...` placeholders.
    Postgres,
}

impl Dialect {
    fn placeholder(self, index: usize) -> String {
        match self {
            Dialect::Postgres => format!("${index}"),
            Dialect::Generic | Dialect::Sqlite | Dialect::MySql => "?".to_string(),
        }
    }

    fn quote_char(self) -> char {
        match self {
            Dialect::MySql => '`',
            Dialect::Generic | Dialect::Sqlite | Dialect::Postgres => '"',
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum IdentifierPolicy {
    /// Do not validate or quote identifiers (legacy behavior).
    Raw,
    /// Validate identifiers against a conservative pattern.
    Validate,
    /// Validate and quote identifiers using the dialect quoting style.
    Quote,
}

#[derive(Debug, Clone)]
pub struct QueryBuilder {
    table: String,
    conditions: Vec<Condition>,
    order: Vec<(String, bool)>,
    limit_val: Option<u64>,
    offset_val: Option<u64>,
    dialect: Dialect,
    identifier_policy: IdentifierPolicy,
}

#[derive(Debug, Clone)]
enum Condition {
    Eq(String, String),
    NotEq(String, String),
    Like(String, String),
    Gt(String, String),
    Lt(String, String),
    In(String, Vec<String>),
    IsNull(String),
    IsNotNull(String),
}

impl QueryBuilder {
    pub fn new(table: &str) -> Self {
        Self {
            table: table.to_string(),
            conditions: Vec::new(),
            order: Vec::new(),
            limit_val: None,
            offset_val: None,
            dialect: Dialect::Generic,
            identifier_policy: IdentifierPolicy::Raw,
        }
    }

    /// Create a new builder with an explicit SQL dialect.
    pub fn new_with_dialect(table: &str, dialect: Dialect) -> Self {
        Self::new(table).dialect(dialect)
    }

    /// Set the SQL dialect (affects placeholder style and quoting).
    pub fn dialect(mut self, dialect: Dialect) -> Self {
        self.dialect = dialect;
        self
    }

    /// Configure identifier validation/quoting behavior.
    pub fn identifier_policy(mut self, policy: IdentifierPolicy) -> Self {
        self.identifier_policy = policy;
        self
    }

    pub fn where_eq(mut self, column: &str, value: &str) -> Self {
        self.conditions
            .push(Condition::Eq(column.to_string(), value.to_string()));
        self
    }

    pub fn where_not_eq(mut self, column: &str, value: &str) -> Self {
        self.conditions
            .push(Condition::NotEq(column.to_string(), value.to_string()));
        self
    }

    pub fn where_like(mut self, column: &str, pattern: &str) -> Self {
        self.conditions
            .push(Condition::Like(column.to_string(), pattern.to_string()));
        self
    }

    pub fn where_gt(mut self, column: &str, value: &str) -> Self {
        self.conditions
            .push(Condition::Gt(column.to_string(), value.to_string()));
        self
    }

    pub fn where_lt(mut self, column: &str, value: &str) -> Self {
        self.conditions
            .push(Condition::Lt(column.to_string(), value.to_string()));
        self
    }

    pub fn where_in(mut self, column: &str, values: &[&str]) -> Self {
        self.conditions.push(Condition::In(
            column.to_string(),
            values.iter().map(|s| s.to_string()).collect(),
        ));
        self
    }

    pub fn where_null(mut self, column: &str) -> Self {
        self.conditions
            .push(Condition::IsNull(column.to_string()));
        self
    }

    pub fn where_not_null(mut self, column: &str) -> Self {
        self.conditions
            .push(Condition::IsNotNull(column.to_string()));
        self
    }

    pub fn order_by(mut self, column: &str, ascending: bool) -> Self {
        self.order.push((column.to_string(), ascending));
        self
    }

    pub fn limit(mut self, limit: u64) -> Self {
        self.limit_val = Some(limit);
        self
    }

    pub fn offset(mut self, offset: u64) -> Self {
        self.offset_val = Some(offset);
        self
    }

    /// Build a SELECT query returning `(sql, bind_values)`.
    ///
    /// The `columns` parameter determines which columns to select (e.g., `"*"` or `"id, name"`).
    pub fn build_select(&self, columns: &str) -> (String, Vec<String>) {
        let mut sql = format!("SELECT {columns} FROM {}", self.table);
        let mut params = Vec::new();
        let mut placeholder_idx = 1usize;
        self.append_where(&mut sql, &mut params, &mut placeholder_idx);
        self.append_order(&mut sql);
        self.append_limit_offset(&mut sql);
        (sql, params)
    }

    /// Build a SELECT query with validated identifiers.
    ///
    /// This method rejects invalid identifiers and optionally quotes them
    /// (depending on `identifier_policy`). Use it when identifiers can come
    /// from user input or when portability matters.
    pub fn build_select_checked(&self, columns: &[&str]) -> Result<(String, Vec<String>), QueryError> {
        let table = self.format_identifier_checked(&self.table, false, "table")?;
        let columns = self.format_column_list_checked(columns)?;

        let mut sql = format!("SELECT {columns} FROM {table}");
        let mut params = Vec::new();
        let mut placeholder_idx = 1usize;
        self.append_where_checked(&mut sql, &mut params, &mut placeholder_idx)?;
        self.append_order_checked(&mut sql)?;
        self.append_limit_offset(&mut sql);
        Ok((sql, params))
    }

    /// Build a COUNT query returning `(sql, bind_values)`.
    pub fn build_count(&self) -> (String, Vec<String>) {
        let mut sql = format!("SELECT COUNT(*) FROM {}", self.table);
        let mut params = Vec::new();
        let mut placeholder_idx = 1usize;
        self.append_where(&mut sql, &mut params, &mut placeholder_idx);
        (sql, params)
    }

    /// Build a COUNT query with validated identifiers.
    pub fn build_count_checked(&self) -> Result<(String, Vec<String>), QueryError> {
        let table = self.format_identifier_checked(&self.table, false, "table")?;
        let mut sql = format!("SELECT COUNT(*) FROM {table}");
        let mut params = Vec::new();
        let mut placeholder_idx = 1usize;
        self.append_where_checked(&mut sql, &mut params, &mut placeholder_idx)?;
        Ok((sql, params))
    }

    fn append_where(&self, sql: &mut String, params: &mut Vec<String>, placeholder_idx: &mut usize) {
        if self.conditions.is_empty() {
            return;
        }
        sql.push_str(" WHERE ");
        let mut first = true;
        for cond in &self.conditions {
            if !first {
                sql.push_str(" AND ");
            }
            first = false;
            match cond {
                Condition::Eq(col, val) => {
                    let placeholder = self.dialect.placeholder(*placeholder_idx);
                    *placeholder_idx += 1;
                    sql.push_str(&format!("{col} = {placeholder}"));
                    params.push(val.clone());
                }
                Condition::NotEq(col, val) => {
                    let placeholder = self.dialect.placeholder(*placeholder_idx);
                    *placeholder_idx += 1;
                    sql.push_str(&format!("{col} != {placeholder}"));
                    params.push(val.clone());
                }
                Condition::Like(col, pat) => {
                    let placeholder = self.dialect.placeholder(*placeholder_idx);
                    *placeholder_idx += 1;
                    sql.push_str(&format!("{col} LIKE {placeholder}"));
                    params.push(pat.clone());
                }
                Condition::Gt(col, val) => {
                    let placeholder = self.dialect.placeholder(*placeholder_idx);
                    *placeholder_idx += 1;
                    sql.push_str(&format!("{col} > {placeholder}"));
                    params.push(val.clone());
                }
                Condition::Lt(col, val) => {
                    let placeholder = self.dialect.placeholder(*placeholder_idx);
                    *placeholder_idx += 1;
                    sql.push_str(&format!("{col} < {placeholder}"));
                    params.push(val.clone());
                }
                Condition::In(col, vals) => {
                    let placeholders: Vec<_> = vals
                        .iter()
                        .map(|_| {
                            let placeholder = self.dialect.placeholder(*placeholder_idx);
                            *placeholder_idx += 1;
                            placeholder
                        })
                        .collect();
                    sql.push_str(&format!("{col} IN ({})", placeholders.join(", ")));
                    params.extend(vals.clone());
                }
                Condition::IsNull(col) => {
                    sql.push_str(&format!("{col} IS NULL"));
                }
                Condition::IsNotNull(col) => {
                    sql.push_str(&format!("{col} IS NOT NULL"));
                }
            }
        }
    }

    fn append_order(&self, sql: &mut String) {
        if self.order.is_empty() {
            return;
        }
        sql.push_str(" ORDER BY ");
        let clauses: Vec<_> = self
            .order
            .iter()
            .map(|(col, asc)| {
                if *asc {
                    format!("{col} ASC")
                } else {
                    format!("{col} DESC")
                }
            })
            .collect();
        sql.push_str(&clauses.join(", "));
    }

    fn append_where_checked(
        &self,
        sql: &mut String,
        params: &mut Vec<String>,
        placeholder_idx: &mut usize,
    ) -> Result<(), QueryError> {
        if self.conditions.is_empty() {
            return Ok(());
        }
        sql.push_str(" WHERE ");
        let mut first = true;
        for cond in &self.conditions {
            if !first {
                sql.push_str(" AND ");
            }
            first = false;
            match cond {
                Condition::Eq(col, val) => {
                    let col = self.format_identifier_checked(col, false, "column")?;
                    let placeholder = self.dialect.placeholder(*placeholder_idx);
                    *placeholder_idx += 1;
                    sql.push_str(&format!("{col} = {placeholder}"));
                    params.push(val.clone());
                }
                Condition::NotEq(col, val) => {
                    let col = self.format_identifier_checked(col, false, "column")?;
                    let placeholder = self.dialect.placeholder(*placeholder_idx);
                    *placeholder_idx += 1;
                    sql.push_str(&format!("{col} != {placeholder}"));
                    params.push(val.clone());
                }
                Condition::Like(col, pat) => {
                    let col = self.format_identifier_checked(col, false, "column")?;
                    let placeholder = self.dialect.placeholder(*placeholder_idx);
                    *placeholder_idx += 1;
                    sql.push_str(&format!("{col} LIKE {placeholder}"));
                    params.push(pat.clone());
                }
                Condition::Gt(col, val) => {
                    let col = self.format_identifier_checked(col, false, "column")?;
                    let placeholder = self.dialect.placeholder(*placeholder_idx);
                    *placeholder_idx += 1;
                    sql.push_str(&format!("{col} > {placeholder}"));
                    params.push(val.clone());
                }
                Condition::Lt(col, val) => {
                    let col = self.format_identifier_checked(col, false, "column")?;
                    let placeholder = self.dialect.placeholder(*placeholder_idx);
                    *placeholder_idx += 1;
                    sql.push_str(&format!("{col} < {placeholder}"));
                    params.push(val.clone());
                }
                Condition::In(col, vals) => {
                    let col = self.format_identifier_checked(col, false, "column")?;
                    let placeholders: Vec<_> = vals
                        .iter()
                        .map(|_| {
                            let placeholder = self.dialect.placeholder(*placeholder_idx);
                            *placeholder_idx += 1;
                            placeholder
                        })
                        .collect();
                    sql.push_str(&format!("{col} IN ({})", placeholders.join(", ")));
                    params.extend(vals.clone());
                }
                Condition::IsNull(col) => {
                    let col = self.format_identifier_checked(col, false, "column")?;
                    sql.push_str(&format!("{col} IS NULL"));
                }
                Condition::IsNotNull(col) => {
                    let col = self.format_identifier_checked(col, false, "column")?;
                    sql.push_str(&format!("{col} IS NOT NULL"));
                }
            }
        }
        Ok(())
    }

    fn append_order_checked(&self, sql: &mut String) -> Result<(), QueryError> {
        if self.order.is_empty() {
            return Ok(());
        }
        sql.push_str(" ORDER BY ");
        let mut clauses = Vec::with_capacity(self.order.len());
        for (col, asc) in &self.order {
            let col = self.format_identifier_checked(col, false, "column")?;
            if *asc {
                clauses.push(format!("{col} ASC"));
            } else {
                clauses.push(format!("{col} DESC"));
            }
        }
        sql.push_str(&clauses.join(", "));
        Ok(())
    }

    fn append_limit_offset(&self, sql: &mut String) {
        if let Some(limit) = self.limit_val {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        if let Some(offset) = self.offset_val {
            sql.push_str(&format!(" OFFSET {offset}"));
        }
    }

    fn format_column_list_checked(&self, columns: &[&str]) -> Result<String, QueryError> {
        let mut out = Vec::with_capacity(columns.len());
        for col in columns {
            out.push(self.format_identifier_checked(col, true, "column")?);
        }
        Ok(out.join(", "))
    }

    fn format_identifier_checked(
        &self,
        ident: &str,
        allow_star: bool,
        kind: &'static str,
    ) -> Result<String, QueryError> {
        if !is_valid_identifier(ident, allow_star) {
            return Err(QueryError::InvalidIdentifier {
                kind,
                ident: ident.to_string(),
            });
        }
        match self.identifier_policy {
            IdentifierPolicy::Quote => Ok(quote_identifier(ident, self.dialect, allow_star)),
            IdentifierPolicy::Raw | IdentifierPolicy::Validate => Ok(ident.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub enum QueryError {
    InvalidIdentifier { kind: &'static str, ident: String },
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::InvalidIdentifier { kind, ident } => {
                write!(f, "Invalid {kind} identifier: {ident}")
            }
        }
    }
}

impl std::error::Error for QueryError {}

fn is_valid_identifier(ident: &str, allow_star: bool) -> bool {
    if ident.is_empty() {
        return false;
    }
    let parts: Vec<&str> = ident.split('.').collect();
    for (idx, part) in parts.iter().enumerate() {
        if allow_star && *part == "*" {
            return idx + 1 == parts.len();
        }
        if !is_valid_segment(part) {
            return false;
        }
    }
    true
}

fn is_valid_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
    }
    true
}

fn quote_identifier(ident: &str, dialect: Dialect, allow_star: bool) -> String {
    let quote = dialect.quote_char();
    let parts: Vec<&str> = ident.split('.').collect();
    let last_idx = parts.len().saturating_sub(1);
    parts
        .into_iter()
        .enumerate()
        .map(|(idx, part)| {
            if allow_star && part == "*" && idx == last_idx {
                part.to_string()
            } else {
                format!("{quote}{part}{quote}")
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_select() {
        let (sql, params) = QueryBuilder::new("users").build_select("*");
        assert_eq!(sql, "SELECT * FROM users");
        assert!(params.is_empty());
    }

    #[test]
    fn test_where_eq() {
        let (sql, params) = QueryBuilder::new("users")
            .where_eq("email", "a@b.com")
            .build_select("*");
        assert_eq!(sql, "SELECT * FROM users WHERE email = ?");
        assert_eq!(params, vec!["a@b.com"]);
    }

    #[test]
    fn test_complex_query() {
        let (sql, params) = QueryBuilder::new("users")
            .where_eq("status", "active")
            .where_like("name", "%alice%")
            .order_by("id", true)
            .limit(10)
            .offset(20)
            .build_select("id, name");
        assert_eq!(
            sql,
            "SELECT id, name FROM users WHERE status = ? AND name LIKE ? ORDER BY id ASC LIMIT 10 OFFSET 20"
        );
        assert_eq!(params, vec!["active", "%alice%"]);
    }

    #[test]
    fn test_count_query() {
        let (sql, params) = QueryBuilder::new("users")
            .where_eq("active", "true")
            .build_count();
        assert_eq!(sql, "SELECT COUNT(*) FROM users WHERE active = ?");
        assert_eq!(params, vec!["true"]);
    }

    #[test]
    fn test_postgres_placeholders() {
        let (sql, params) = QueryBuilder::new_with_dialect("users", Dialect::Postgres)
            .where_eq("status", "active")
            .where_in("role", &["admin", "user"])
            .build_select("*");
        assert_eq!(
            sql,
            "SELECT * FROM users WHERE status = $1 AND role IN ($2, $3)"
        );
        assert_eq!(params, vec!["active", "admin", "user"]);
    }

    #[test]
    fn test_checked_identifiers_and_quoting() {
        let (sql, params) = QueryBuilder::new("users")
            .dialect(Dialect::Postgres)
            .identifier_policy(IdentifierPolicy::Quote)
            .where_eq("users.email", "a@b.com")
            .order_by("users.id", true)
            .build_select_checked(&["users.id", "users.email"])
            .unwrap();
        assert_eq!(
            sql,
            "SELECT \"users\".\"id\", \"users\".\"email\" FROM \"users\" WHERE \"users\".\"email\" = $1 ORDER BY \"users\".\"id\" ASC"
        );
        assert_eq!(params, vec!["a@b.com"]);
    }

    #[test]
    fn test_checked_invalid_identifier() {
        let err = QueryBuilder::new("users;drop")
            .build_select_checked(&["*"])
            .unwrap_err();
        assert!(matches!(err, QueryError::InvalidIdentifier { .. }));
    }
}
