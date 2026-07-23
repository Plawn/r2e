use r2e::prelude::*;

#[derive(Clone)]
pub struct Greeter {
    greeting: String,
}

#[bean]
impl Greeter {
    pub fn new(#[config("app.greeting")] greeting: String) -> Self {
        Self { greeting }
    }
}

impl Greeter {
    pub fn greet(&self, name: &str) -> String {
        format!("{}, {}!", self.greeting, name)
    }
}

fn main() {}
