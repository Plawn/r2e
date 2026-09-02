//! Scaffolding for `llm/validation.md`.

pub use serde::{Deserialize, Serialize};

/// The entity the `Params` handler returns.
#[derive(Serialize, schemars::JsonSchema)]
pub struct User {
    pub id: u64,
    pub name: String,
}

/// Stand-in for the lookup the handler would do.
pub fn user_by_id(id: u64) -> User {
    User { id, name: "ada".into() }
}

/// The search result of the `Query<T>` → `Params` migration example.
#[derive(Default, Serialize, schemars::JsonSchema)]
pub struct Hits {
    pub total: u64,
}

/// Stand-in for the search the handler would run.
pub fn search_hits(_query: &str, _page_size: Option<u32>) -> Hits {
    Hits::default()
}
