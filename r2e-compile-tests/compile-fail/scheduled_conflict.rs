use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[derive(Controller)]
#[controller(state = AppState)]
pub struct Jobs;

#[routes]
impl Jobs {
    #[scheduled(every = 5, cron = "0 */5 * * * *")]
    async fn my_task(&self) {
        println!("tick");
    }
}

fn main() {}
