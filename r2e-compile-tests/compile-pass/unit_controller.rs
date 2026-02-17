use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[derive(Controller)]
#[controller(path = "/unit", state = AppState)]
pub struct UnitController;

#[routes]
impl UnitController {
    #[get("/ping")]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

fn main() {}
