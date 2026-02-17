use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

pub struct MyResource;

#[derive(Controller)]
#[controller(path = "/test", state = AppState)]
pub struct MyController;

#[routes]
impl MyController {
    #[post("/")]
    async fn create(&self, #[managed] res: MyResource) -> &'static str {
        "created"
    }
}

fn main() {}
