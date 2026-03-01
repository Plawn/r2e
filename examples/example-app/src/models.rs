use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use garde::Validate;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
}

#[derive(Deserialize, Validate, JsonSchema)]
pub struct CreateUserRequest {
    #[garde(length(min = 1, max = 100))]
    pub name: String,
    #[garde(email)]
    pub email: String,
}

/// Entity for the `users` SQL table, used by the data controller.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserEntity {
    pub id: i64,
    pub name: String,
    pub email: String,
}

impl r2e::r2e_data::Entity for UserEntity {
    type Id = i64;
    fn table_name() -> &'static str {
        "users"
    }
    fn id_column() -> &'static str {
        "id"
    }
    fn columns() -> &'static [&'static str] {
        &["id", "name", "email"]
    }
    fn id(&self) -> &i64 {
        &self.id
    }
}

/// Event emitted when a new user is created.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserCreatedEvent {
    pub user_id: u64,
    pub name: String,
    pub email: String,
}
