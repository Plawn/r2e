use std::sync::Arc;

use r2e::config::R2eConfig;
use r2e::prelude::*;
use r2e::r2e_security::{AuthenticatedUser, JwtClaimsValidator};
use r2e_test::{TestApp, TestJwt};
use tokio::sync::RwLock;

// ─── Types ───

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Item {
    pub id: u64,
    pub name: String,
    pub active: bool,
}

#[derive(Clone)]
pub struct ItemService {
    items: Arc<RwLock<Vec<Item>>>,
}

impl ItemService {
    fn new() -> Self {
        let items = vec![
            Item { id: 1, name: "First".into(), active: true },
            Item { id: 2, name: "Second".into(), active: true },
            Item { id: 3, name: "Third".into(), active: false },
        ];
        Self {
            items: Arc::new(RwLock::new(items)),
        }
    }

    pub async fn list(&self) -> Vec<Item> {
        self.items.read().await.clone()
    }

    pub async fn get(&self, id: u64) -> Option<Item> {
        self.items.read().await.iter().find(|i| i.id == id).cloned()
    }

    pub async fn update(&self, id: u64, name: String) -> Option<Item> {
        let mut items = self.items.write().await;
        if let Some(item) = items.iter_mut().find(|i| i.id == id) {
            item.name = name;
            Some(item.clone())
        } else {
            None
        }
    }

    pub async fn patch(&self, id: u64, active: Option<bool>) -> Option<Item> {
        let mut items = self.items.write().await;
        if let Some(item) = items.iter_mut().find(|i| i.id == id) {
            if let Some(active) = active {
                item.active = active;
            }
            Some(item.clone())
        } else {
            None
        }
    }

    pub async fn delete(&self, id: u64) -> bool {
        let mut items = self.items.write().await;
        let len_before = items.len();
        items.retain(|i| i.id != id);
        items.len() < len_before
    }
}

// ─── State ───

#[derive(Clone)]
struct VerbTestState {
    item_service: ItemService,
    jwt_validator: Arc<JwtClaimsValidator>,
    config: R2eConfig,
}

impl r2e::http::extract::FromRef<VerbTestState> for ItemService {
    fn from_ref(state: &VerbTestState) -> Self {
        state.item_service.clone()
    }
}

impl r2e::http::extract::FromRef<VerbTestState> for Arc<JwtClaimsValidator> {
    fn from_ref(state: &VerbTestState) -> Self {
        state.jwt_validator.clone()
    }
}

impl r2e::http::extract::FromRef<VerbTestState> for R2eConfig {
    fn from_ref(state: &VerbTestState) -> Self {
        state.config.clone()
    }
}

// ─── Controller with all HTTP verbs (struct-level identity) ───

#[derive(Controller)]
#[controller(path = "/items", state = VerbTestState)]
pub struct ItemController {
    #[inject]
    item_service: ItemService,

    #[identity]
    user: AuthenticatedUser,
}

#[routes]
impl ItemController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<Item>> {
        Json(self.item_service.list().await)
    }

    #[get("/{id}")]
    async fn get_by_id(
        &self,
        Path(id): Path<u64>,
    ) -> Result<Json<Item>, AppError> {
        self.item_service
            .get(id)
            .await
            .map(Json)
            .ok_or_else(|| AppError::NotFound("Item not found".into()))
    }

    #[put("/{id}")]
    async fn update(
        &self,
        Path(id): Path<u64>,
        Json(body): Json<serde_json::Value>,
    ) -> Result<Json<Item>, AppError> {
        let name = body["name"]
            .as_str()
            .ok_or_else(|| AppError::BadRequest("name required".into()))?;
        self.item_service
            .update(id, name.to_string())
            .await
            .map(Json)
            .ok_or_else(|| AppError::NotFound("Item not found".into()))
    }

    #[delete("/{id}")]
    async fn delete(
        &self,
        Path(id): Path<u64>,
    ) -> Result<StatusCode, AppError> {
        if self.item_service.delete(id).await {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(AppError::NotFound("Item not found".into()))
        }
    }

    #[patch("/{id}")]
    async fn patch(
        &self,
        Path(id): Path<u64>,
        Json(body): Json<serde_json::Value>,
    ) -> Result<Json<Item>, AppError> {
        let active = body["active"].as_bool();
        self.item_service
            .patch(id, active)
            .await
            .map(Json)
            .ok_or_else(|| AppError::NotFound("Item not found".into()))
    }
}

async fn setup() -> (TestApp, TestJwt) {
    let jwt = TestJwt::new();
    let config = R2eConfig::empty();

    let state = VerbTestState {
        item_service: ItemService::new(),
        jwt_validator: Arc::new(jwt.claims_validator()),
        config: config.clone(),
    };

    let app = TestApp::from_builder(
        AppBuilder::new()
            .with_state(state)
            .with_config(config)
            .with(ErrorHandling)
            .register_controller::<ItemController>(),
    );

    (app, jwt)
}

// ─── GET Tests ───

#[tokio::test]
async fn test_get_list_items() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app.get("/items").bearer(&token).send().await.assert_ok();
    let items: Vec<Item> = resp.json();
    assert_eq!(items.len(), 3);
}

#[tokio::test]
async fn test_get_item_by_id() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app.get("/items/1").bearer(&token).send().await.assert_ok();
    let item: Item = resp.json();
    assert_eq!(item.name, "First");
}

#[tokio::test]
async fn test_get_item_not_found() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    app.get("/items/999")
        .bearer(&token)
        .send()
        .await
        .assert_not_found();
}

// ─── PUT Tests ───

#[tokio::test]
async fn test_put_update_item() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let body = serde_json::json!({ "name": "Updated First" });
    let resp = app
        .put("/items/1")
        .json(&body)
        .bearer(&token)
        .send()
        .await
        .assert_ok();
    let item: Item = resp.json();
    assert_eq!(item.id, 1);
    assert_eq!(item.name, "Updated First");
}

#[tokio::test]
async fn test_put_update_not_found() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let body = serde_json::json!({ "name": "Ghost" });
    app.put("/items/999")
        .json(&body)
        .bearer(&token)
        .send()
        .await
        .assert_not_found();
}

// ─── DELETE Tests ───

#[tokio::test]
async fn test_delete_item() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    app.delete("/items/1")
        .bearer(&token)
        .send()
        .await
        .assert_status(http::StatusCode::NO_CONTENT);

    // Verify it's gone
    app.get("/items/1")
        .bearer(&token)
        .send()
        .await
        .assert_not_found();
}

#[tokio::test]
async fn test_delete_not_found() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    app.delete("/items/999")
        .bearer(&token)
        .send()
        .await
        .assert_not_found();
}

// ─── PATCH Tests ───

#[tokio::test]
async fn test_patch_partial_update() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    // Item 3 starts as active=false
    let resp = app.get("/items/3").bearer(&token).send().await.assert_ok();
    let item: Item = resp.json();
    assert!(!item.active);

    // Patch to active=true
    let body = serde_json::json!({ "active": true });
    let resp = app
        .patch("/items/3")
        .json(&body)
        .bearer(&token)
        .send()
        .await
        .assert_ok();
    let item: Item = resp.json();
    assert_eq!(item.id, 3);
    assert!(item.active);
    assert_eq!(item.name, "Third"); // name unchanged
}

#[tokio::test]
async fn test_patch_not_found() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let body = serde_json::json!({ "active": true });
    app.patch("/items/999")
        .json(&body)
        .bearer(&token)
        .send()
        .await
        .assert_not_found();
}
