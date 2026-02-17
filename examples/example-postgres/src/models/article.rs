use r2e::r2e_data::Entity;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use validator::Validate;

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Article {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub published: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Entity for Article {
    type Id = i64;

    fn table_name() -> &'static str {
        "articles"
    }

    fn id_column() -> &'static str {
        "id"
    }

    fn columns() -> &'static [&'static str] {
        &["id", "title", "body", "published", "created_at", "updated_at"]
    }

    fn id(&self) -> &i64 {
        &self.id
    }
}

#[derive(Debug, Deserialize, Validate, JsonSchema)]
pub struct CreateArticleRequest {
    #[validate(length(min = 1, max = 200, message = "Title must be 1-200 characters"))]
    pub title: String,
    #[validate(length(min = 1, message = "Body must not be empty"))]
    pub body: String,
    #[serde(default)]
    pub published: bool,
}

#[derive(Debug, Deserialize, Validate, JsonSchema)]
pub struct UpdateArticleRequest {
    #[validate(length(min = 1, max = 200, message = "Title must be 1-200 characters"))]
    pub title: Option<String>,
    #[validate(length(min = 1, message = "Body must not be empty"))]
    pub body: Option<String>,
    pub published: Option<bool>,
}
