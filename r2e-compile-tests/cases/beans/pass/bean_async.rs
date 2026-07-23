use r2e::prelude::*;

#[derive(Clone)]
pub struct AsyncService {
    data: String,
}

#[bean]
impl AsyncService {
    pub async fn new(#[config("app.data")] data: String) -> Self {
        // Simulate async init
        Self { data }
    }
}

impl AsyncService {
    pub fn data(&self) -> &str {
        &self.data
    }
}

fn main() {}
