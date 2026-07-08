//! The application state is inferred from the provision list: build_state()
//! takes no type arguments, beans are read back by type, and a controller's
//! #[inject] deps are checked against the state at register_controller.

use r2e::prelude::*;
use r2e::BeanAccess;

#[derive(Clone)]
pub struct MyService;

#[controller(path = "/api")]
pub struct ApiController {
    #[inject]
    service: MyService,
}

#[routes]
impl ApiController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        let _ = &self.service;
        "ok"
    }
}

async fn _assemble() {
    let app = AppBuilder::new()
        .provide(MyService)
        .build_state()
        .await;
    let _service: MyService = app.state().get::<MyService>();
    let _app = app.register_controller::<ApiController>();
}

fn main() {}
