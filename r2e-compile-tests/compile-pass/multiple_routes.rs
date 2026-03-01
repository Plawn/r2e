use r2e::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct AppState;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Item {
    pub id: u64,
    pub name: String,
}

#[derive(Controller)]
#[controller(path = "/items", state = AppState)]
pub struct ItemController;

#[routes]
impl ItemController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<Item>> {
        Json(vec![])
    }

    #[get("/{id}")]
    async fn get_by_id(&self, Path(id): Path<u64>) -> Json<Item> {
        Json(Item { id, name: "test".into() })
    }

    #[post("/")]
    async fn create(&self, Json(item): Json<Item>) -> Json<Item> {
        Json(item)
    }

    #[put("/{id}")]
    async fn update(&self, Path(id): Path<u64>, Json(mut item): Json<Item>) -> Json<Item> {
        item.id = id;
        Json(item)
    }

    #[delete("/{id}")]
    async fn delete(&self, Path(_id): Path<u64>) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

fn main() {}
