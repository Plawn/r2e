use r2e::prelude::*;

#[derive(Clone)]
pub struct DbPool {
    url: String,
}

#[producer]
async fn create_pool(#[config("db.url")] url: String) -> DbPool {
    DbPool { url }
}

fn main() {}
