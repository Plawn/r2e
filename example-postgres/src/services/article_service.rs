use r2e::prelude::*;
use r2e::r2e_data::{Page, Pageable};
use sqlx::PgPool;

use crate::error::AppError;
use crate::models::{Article, CreateArticleRequest, UpdateArticleRequest};

#[derive(Clone)]
pub struct ArticleService {
    pool: PgPool,
}

#[bean]
impl ArticleService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn list(&self, pageable: &Pageable) -> Result<Page<Article>, AppError> {
        let offset = pageable.offset();

        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM articles")
            .fetch_one(&self.pool)
            .await?;

        let articles = sqlx::query_as::<_, Article>(
            "SELECT id, title, body, published, created_at, updated_at \
             FROM articles ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(pageable.size as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(Page::new(articles, pageable, total.0 as u64))
    }

    pub async fn get_by_id(&self, id: i64) -> Result<Article, AppError> {
        sqlx::query_as::<_, Article>(
            "SELECT id, title, body, published, created_at, updated_at \
             FROM articles WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("Article {} not found", id)))
    }

    pub async fn create(&self, req: CreateArticleRequest) -> Result<Article, AppError> {
        let article = sqlx::query_as::<_, Article>(
            "INSERT INTO articles (title, body, published) VALUES ($1, $2, $3) \
             RETURNING id, title, body, published, created_at, updated_at",
        )
        .bind(&req.title)
        .bind(&req.body)
        .bind(req.published)
        .fetch_one(&self.pool)
        .await?;

        Ok(article)
    }

    pub async fn update(&self, id: i64, req: UpdateArticleRequest) -> Result<Article, AppError> {
        let existing = self.get_by_id(id).await?;

        let title = req.title.unwrap_or(existing.title);
        let body = req.body.unwrap_or(existing.body);
        let published = req.published.unwrap_or(existing.published);

        let article = sqlx::query_as::<_, Article>(
            "UPDATE articles SET title = $1, body = $2, published = $3, updated_at = NOW() \
             WHERE id = $4 \
             RETURNING id, title, body, published, created_at, updated_at",
        )
        .bind(&title)
        .bind(&body)
        .bind(published)
        .bind(id)
        .fetch_one(&self.pool)
        .await?;

        Ok(article)
    }

    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let result = sqlx::query("DELETE FROM articles WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(AppError::NotFound(format!("Article {} not found", id)));
        }

        Ok(())
    }
}
