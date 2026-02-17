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
    #[transactional]
    async fn create(&self, #[managed] res: &mut MyResource) -> &'static str {
        "created"
    }
}

fn main() {}
