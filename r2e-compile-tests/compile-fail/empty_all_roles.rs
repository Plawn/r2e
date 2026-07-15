use r2e::prelude::*;

#[controller(path = "/test")]
pub struct MyController;

#[routes]
impl MyController {
    #[get("/")]
    #[all_roles("   ")]
    async fn show(&self) -> &'static str {
        "must not compile"
    }
}

fn main() {}
