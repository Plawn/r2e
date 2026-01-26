/// Trait representing a database entity with a table name, id column, and column list.
///
/// Intended to be implemented manually or via a derive macro (`#[derive(Entity)]`).
///
/// # Example
///
/// ```ignore
/// impl Entity for UserEntity {
///     type Id = i64;
///     fn table_name() -> &'static str { "users" }
///     fn id_column() -> &'static str { "id" }
///     fn columns() -> &'static [&'static str] { &["id", "name", "email"] }
///     fn id(&self) -> &i64 { &self.id }
/// }
/// ```
pub trait Entity: Send + Sync + Unpin + 'static {
    type Id: Send + Sync + ToString + 'static;

    fn table_name() -> &'static str;
    fn id_column() -> &'static str;
    fn columns() -> &'static [&'static str];
    fn id(&self) -> &Self::Id;
}
