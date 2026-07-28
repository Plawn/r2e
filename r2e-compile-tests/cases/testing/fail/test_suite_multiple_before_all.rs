//! A suite has one suite-level initializer.

#[derive(Default)]
struct BadSuite;

#[r2e::test_suite]
impl BadSuite {
    #[before_all]
    fn one() {}

    #[before_all]
    fn two() {}

    #[case]
    fn case(&mut self) {}
}

fn main() {}
