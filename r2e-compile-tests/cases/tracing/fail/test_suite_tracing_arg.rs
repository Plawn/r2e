//! `#[r2e::test_suite(tracing = false)]` was removed; the error names the
//! migration page.

#[derive(Default)]
struct Suite;

#[r2e::test_suite(tracing = false)]
impl Suite {
    #[case]
    fn runs(&mut self) {}
}

fn main() {}
