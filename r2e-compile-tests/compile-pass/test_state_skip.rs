//! #[derive(TestState)] generates FromRef + bean-lookup bridge impls per
//! field; #[test_state(skip)] suppresses them for a field.

use r2e::prelude::*;

#[derive(Clone)]
pub struct MyService;

#[derive(Clone, TestState)]
pub struct TestHarnessState {
    pub service: MyService,
    #[test_state(skip)]
    pub internal: String,
}

fn main() {}
