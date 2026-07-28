//! Suite case orders must be unique within one `impl`.

#[derive(Default)]
struct BadSuite;

#[r2e::test_suite]
impl BadSuite {
    #[case(order = 1)]
    fn first(&mut self) {}

    #[case(order = 1)]
    fn second(&mut self) {}
}

fn main() {}
