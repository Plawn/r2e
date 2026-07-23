use r2e::prelude::*;

#[derive(Clone)]
pub struct MyService;

#[bean]
impl MyService {
    pub fn new(&self) -> Self {
        Self
    }
}

fn main() {}
