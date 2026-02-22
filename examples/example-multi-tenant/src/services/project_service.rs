use r2e::prelude::*;
use sqlx::SqlitePool;

use crate::models::{CreateProjectRequest, Project, TenantInfo};

#[derive(Clone)]
pub struct ProjectService {
    pool: SqlitePool,
}

#[bean]
impl ProjectService {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn list_by_tenant(&self, tenant_id: &str) -> Result<Vec<Project>, HttpError> {
        let projects = sqlx::query_as::<_, Project>(
            "SELECT id, tenant_id, name, description FROM projects WHERE tenant_id = ?",
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| HttpError::Internal(e.to_string()))?;

        Ok(projects)
    }

    pub async fn create(
        &self,
        tenant_id: &str,
        req: CreateProjectRequest,
    ) -> Result<Project, HttpError> {
        let project = sqlx::query_as::<_, Project>(
            "INSERT INTO projects (tenant_id, name, description) VALUES (?, ?, ?) \
             RETURNING id, tenant_id, name, description",
        )
        .bind(tenant_id)
        .bind(&req.name)
        .bind(&req.description)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| HttpError::Internal(e.to_string()))?;

        Ok(project)
    }

    pub async fn list_tenants(&self) -> Result<Vec<TenantInfo>, HttpError> {
        let tenants: Vec<(String, i64)> = sqlx::query_as(
            "SELECT tenant_id, COUNT(*) as project_count \
             FROM projects GROUP BY tenant_id ORDER BY tenant_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| HttpError::Internal(e.to_string()))?;

        Ok(tenants
            .into_iter()
            .map(|(tenant_id, project_count)| TenantInfo {
                tenant_id,
                project_count,
            })
            .collect())
    }
}
