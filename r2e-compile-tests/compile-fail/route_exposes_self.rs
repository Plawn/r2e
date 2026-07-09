//! Phase 4 diagnostic: a route method may not expose `Self` in its signature.
//! Route methods are moved onto the generated request façade, where `Self` would
//! silently mean the hidden façade type rather than the controller.

use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState {
    pub name: String,
}

#[controller(path = "/x")]
pub struct MyController {
    #[inject]
    name: String,
}

#[routes]
impl MyController {
    #[get("/clone")]
    async fn clone_me(&self) -> Json<Self> {
        todo!()
    }
}

fn main() {}
