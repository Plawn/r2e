//! `#[anonymous]` opts a route out of the controller's struct-level identity.
//! Without one, every route is already public — the marker is redundant and
//! rejected (it would silently move the method to the core otherwise).

use r2e::prelude::*;

#[controller(path = "/test")]
pub struct MyController;

#[routes]
impl MyController {
    #[get("/")]
    #[anonymous]
    async fn show(&self) -> Json<String> {
        Json("already public".to_string())
    }
}

fn main() {}
