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
#[derive(Debug, Clone)]
pub struct QueryBuilder {
    table: String,
    conditions: Vec<Condition>,
    order: Vec<(String, bool)>,
    limit_val: Option<u64>,
    offset_val: Option<u64>,
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
        }
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
        self.append_where(&mut sql, &mut params);
        self.append_order(&mut sql);
        self.append_limit_offset(&mut sql);
        (sql, params)
    }

    /// Build a COUNT query returning `(sql, bind_values)`.
    pub fn build_count(&self) -> (String, Vec<String>) {
        let mut sql = format!("SELECT COUNT(*) FROM {}", self.table);
        let mut params = Vec::new();
        self.append_where(&mut sql, &mut params);
        (sql, params)
    }

    fn append_where(&self, sql: &mut String, params: &mut Vec<String>) {
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
                    sql.push_str(&format!("{col} = ?"));
                    params.push(val.clone());
                }
                Condition::NotEq(col, val) => {
                    sql.push_str(&format!("{col} != ?"));
                    params.push(val.clone());
                }
                Condition::Like(col, pat) => {
                    sql.push_str(&format!("{col} LIKE ?"));
                    params.push(pat.clone());
                }
                Condition::Gt(col, val) => {
                    sql.push_str(&format!("{col} > ?"));
                    params.push(val.clone());
                }
                Condition::Lt(col, val) => {
                    sql.push_str(&format!("{col} < ?"));
                    params.push(val.clone());
                }
                Condition::In(col, vals) => {
                    let placeholders: Vec<_> = vals.iter().map(|_| "?").collect();
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

    fn append_limit_offset(&self, sql: &mut String) {
        if let Some(limit) = self.limit_val {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        if let Some(offset) = self.offset_val {
            sql.push_str(&format!(" OFFSET {offset}"));
        }
    }
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
}
