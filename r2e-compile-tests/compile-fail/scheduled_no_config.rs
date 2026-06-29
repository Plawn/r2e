use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller(state = AppState)]
pub struct Jobs;

#[routes]
impl Jobs {
    #[scheduled]
    async fn my_task(&self) {
        println!("tick");
    }
}

fn main() {}
