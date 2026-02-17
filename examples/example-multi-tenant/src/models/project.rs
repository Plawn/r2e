use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Project {
    pub id: i64,
    pub tenant_id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateProjectRequest {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct TenantInfo {
    pub tenant_id: String,
    pub project_count: i64,
}
