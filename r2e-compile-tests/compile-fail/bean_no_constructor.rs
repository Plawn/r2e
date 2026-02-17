use r2e::prelude::*;

#[derive(Clone)]
pub struct MyService;

#[bean]
impl MyService {
    pub fn hello(&self) -> String {
        "hello".into()
    }
}

fn main() {}
