use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState {
    pub greeting: String,
}

#[controller(path = "/api")]
pub struct BasicController {
    #[inject]
    greeting: String,
}

#[routes]
impl BasicController {
    #[get("/hello")]
    async fn hello(&self) -> String {
        self.greeting.clone()
    }

    #[post("/echo")]
    async fn echo(&self, body: String) -> String {
        body
    }
}

fn main() {}
