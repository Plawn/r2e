use r2e::prelude::*;

pub struct Factory;

impl Factory {
    #[producer]
    fn create(&self) -> String {
        "hello".into()
    }
}

fn main() {}
