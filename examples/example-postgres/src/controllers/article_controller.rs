use r2e::prelude::*;
use r2e::r2e_data::{Page, Pageable};
use r2e::r2e_utils::interceptors::Logged;

use crate::error::AppError;
use crate::models::{Article, CreateArticleRequest, UpdateArticleRequest};
use crate::services::ArticleService;
use crate::state::AppState;

#[derive(Controller)]
#[controller(path = "/articles", state = AppState)]
pub struct ArticleController {
    #[inject]
    article_service: ArticleService,
}

#[routes]
#[intercept(Logged::info())]
impl ArticleController {
    #[get("/")]
    async fn list(
        &self,
        Query(pageable): Query<Pageable>,
    ) -> Result<Json<Page<Article>>, AppError> {
        let page = self.article_service.list(&pageable).await?;
        Ok(Json(page))
    }

    #[get("/{id}")]
    async fn get_by_id(&self, Path(id): Path<i64>) -> Result<Json<Article>, AppError> {
        let article = self.article_service.get_by_id(id).await?;
        Ok(Json(article))
    }

    #[post("/")]
    async fn create(
        &self,
        Json(body): Json<CreateArticleRequest>,
    ) -> Result<Json<Article>, AppError> {
        let article = self.article_service.create(body).await?;
        Ok(Json(article))
    }

    #[put("/{id}")]
    async fn update(
        &self,
        Path(id): Path<i64>,
        Json(body): Json<UpdateArticleRequest>,
    ) -> Result<Json<Article>, AppError> {
        let article = self.article_service.update(id, body).await?;
        Ok(Json(article))
    }

    #[delete("/{id}")]
    async fn delete(&self, Path(id): Path<i64>) -> Result<(), AppError> {
        self.article_service.delete(id).await
    }
}
