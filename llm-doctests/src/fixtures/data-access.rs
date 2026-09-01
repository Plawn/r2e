//! Scaffolding for `llm/data-access.md`.

use serde::Serialize;

/// The row type the pagination example pages over.
#[derive(Serialize)]
pub struct UserEntity {
    pub id: i64,
    pub name: String,
    pub email: String,
}

/// The page of rows a repository call produced.
#[allow(non_upper_case_globals)]
pub const entities: Vec<UserEntity> = Vec::new();

/// The matching total-row count.
#[allow(non_upper_case_globals)]
pub const total: u64 = 0;
