//! `#[method]` requires the JSON-RPC method name — a bare marker must be
//! rejected with an error showing the expected form.

use r2e::prelude::*;

#[controller]
pub struct NamelessMethods {}

#[routes]
impl NamelessMethods {
    /// Broken: no `name`.
    #[method]
    async fn broken(&self) -> Json<u32> {
        Json(1)
    }
}

fn main() {}
