//! A suite without cases is almost certainly a mistaken annotation.

#[derive(Default)]
struct BadSuite;

#[r2e::test_suite]
impl BadSuite {
    fn helper(&mut self) {}
}

fn main() {}
