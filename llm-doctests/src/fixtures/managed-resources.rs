//! Scaffolding for `llm/managed-resources.md`.

use r2e::prelude::*;

pub use serde::{Deserialize, Serialize};

/// Diesel query-builder methods (`.execute(conn)`) come from this trait.
pub use diesel::RunQueryDsl;

/// Database markers/connections the snippets name.
pub use diesel::{PgConnection, SqliteConnection};
pub use sqlx::Sqlite;

/// The migration set the Diesel datasource snippet attaches.
pub use diesel_migrations::{EmbeddedMigrations, embed_migrations};

/// Request body and response of the `#[post("/db")]` snippets.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct CreateUser {
    pub name: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct User {
    pub name: String,
}

/// The bean the `SqlxDataSource` snippet registers — it injects the rotating
/// pool the plugin provides.
#[derive(Clone)]
pub struct ArticleService {
    _pool: r2e::r2e_data_sqlx::DbPool<sqlx::Postgres>,
}

#[bean]
impl ArticleService {
    pub fn new(pool: r2e::r2e_data_sqlx::DbPool<sqlx::Postgres>) -> Self {
        Self { _pool: pool }
    }
}

// The Diesel schema the last snippet inserts into.
diesel::table! {
    users (id) {
        id -> Integer,
        name -> Text,
    }
}

#[derive(diesel::Insertable)]
#[diesel(table_name = users)]
pub struct NewUser {
    pub name: String,
}
