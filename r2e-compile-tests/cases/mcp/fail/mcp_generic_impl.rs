use r2e::prelude::*;

pub struct GenericTools<T>(T);

#[mcp_routes]
impl<T> GenericTools<T> {
    #[tool]
    async fn ping(&self) -> String {
        "pong".to_string()
    }
}

fn main() {}
