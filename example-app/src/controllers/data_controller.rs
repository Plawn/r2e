use crate::models::UserEntity;
use crate::state::Services;
use quarlus::prelude::*;
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
        let count_qb = QueryBuilder::new(UserEntity::table_name());
        let (count_sql, count_params) = count_qb.build_count();

        let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);
        for p in &count_params {
            count_query = count_query.bind(p);
        }
        let total = count_query
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))? as u64;

        let select_qb = QueryBuilder::new(UserEntity::table_name())
            .order_by("id", true)
            .limit(pageable.size)
            .offset(pageable.offset());
        let (sql, params) = select_qb.build_select("id, name, email");

        let mut select_query = sqlx::query_as::<_, (i64, String, String)>(&sql);
        for p in &params {
            select_query = select_query.bind(p);
        }
        let rows = select_query
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
        let mut qb = QueryBuilder::new(UserEntity::table_name());
        if let Some(ref name) = params.name {
            qb = qb.where_like("name", &format!("%{name}%"));
        }
        if let Some(ref email) = params.email {
            qb = qb.where_eq("email", email);
        }
        let (sql, bind_params) = qb.order_by("id", true).build_select("id, name, email");

        let mut query = sqlx::query_as::<_, (i64, String, String)>(&sql);
        for p in &bind_params {
            query = query.bind(p);
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
