use r2e::r2e_data::Entity;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use garde::Validate;

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
    #[garde(length(min = 1, max = 200))]
    pub title: String,
    #[garde(length(min = 1))]
    pub body: String,
    #[garde(skip)]
    #[serde(default)]
    pub published: bool,
}

#[derive(Debug, Deserialize, Validate, JsonSchema)]
pub struct UpdateArticleRequest {
    #[garde(length(min = 1, max = 200))]
    pub title: Option<String>,
    #[garde(length(min = 1))]
    pub body: Option<String>,
    #[garde(skip)]
    pub published: Option<bool>,
}
