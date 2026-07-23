use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller]
pub struct Jobs;

#[routes]
impl Jobs {
    #[scheduled(every = 5, cron = "0 */5 * * * *")]
    async fn my_task(&self) {
        println!("tick");
    }
}

fn main() {}
