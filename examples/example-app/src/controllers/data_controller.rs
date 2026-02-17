use crate::models::UserEntity;
use crate::state::Services;
use r2e::prelude::*;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct SearchParams {
    pub name: Option<String>,
    pub email: Option<String>,
}

#[derive(Controller)]
#[controller(path = "/data/users", state = Services)]
pub struct DataController {
    #[inject]
    pool: sqlx::SqlitePool,
}

#[routes]
impl DataController {
    #[get("/")]
    async fn list_paged(
        &self,
        Query(pageable): Query<Pageable>,
    ) -> Result<Json<Page<UserEntity>>, AppError> {
        let total = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))? as u64;

        let rows = sqlx::query_as::<_, (i64, String, String)>(
            "SELECT id, name, email FROM users ORDER BY id ASC LIMIT ? OFFSET ?",
        )
        .bind(pageable.size as i64)
        .bind(pageable.offset() as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        let entities: Vec<UserEntity> = rows
            .into_iter()
            .map(|(id, name, email)| UserEntity { id, name, email })
            .collect();

        Ok(Json(Page::new(entities, &pageable, total)))
    }

    #[get("/search")]
    async fn search(
        &self,
        Query(params): Query<SearchParams>,
    ) -> Result<Json<Vec<UserEntity>>, AppError> {
        let mut sql = String::from("SELECT id, name, email FROM users WHERE 1=1");
        let mut bind_name: Option<String> = None;
        let mut bind_email: Option<&str> = None;

        if let Some(ref name) = params.name {
            sql.push_str(" AND name LIKE ?");
            bind_name = Some(format!("%{name}%"));
        }
        if let Some(ref email) = params.email {
            sql.push_str(" AND email = ?");
            bind_email = Some(email);
        }
        sql.push_str(" ORDER BY id ASC");

        let mut query = sqlx::query_as::<_, (i64, String, String)>(&sql);
        if let Some(ref pattern) = bind_name {
            query = query.bind(pattern);
        }
        if let Some(email) = bind_email {
            query = query.bind(email);
        }

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        let entities: Vec<UserEntity> = rows
            .into_iter()
            .map(|(id, name, email)| UserEntity { id, name, email })
            .collect();

        Ok(Json(entities))
    }
}
