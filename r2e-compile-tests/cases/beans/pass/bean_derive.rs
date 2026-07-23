use r2e::prelude::*;

#[derive(Clone)]
pub struct DepService;

#[bean]
impl DepService {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Clone, Bean)]
pub struct MyService {
    #[inject]
    dep: DepService,
    #[config("app.name")]
    name: String,
}

fn main() {}
