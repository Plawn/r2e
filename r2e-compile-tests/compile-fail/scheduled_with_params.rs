use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[derive(Controller)]
#[controller(state = AppState)]
pub struct Jobs;

#[routes]
impl Jobs {
    #[scheduled(every = 10)]
    async fn my_task(&self, name: String) {
        println!("{}", name);
    }
}

fn main() {}
