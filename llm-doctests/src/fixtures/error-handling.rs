//! Scaffolding for `llm/error-handling.md`.

/// The row the repository returns.
#[derive(Clone, Default)]
pub struct User {
    pub id: i64,
}

/// The payload handed to `db.insert(&u)`.
#[derive(Clone, Default)]
pub struct NewUser {
    pub email: String,
}

/// The repository the `.http_context(...)` example calls — its error type is
/// one `HttpError` has a `From` impl for, which is what `HttpErrorExt`
/// requires.
#[derive(Clone, Default)]
pub struct Db;

impl Db {
    pub async fn insert(&self, _user: &NewUser) -> Result<User, std::io::Error> {
        Ok(User::default())
    }
}
