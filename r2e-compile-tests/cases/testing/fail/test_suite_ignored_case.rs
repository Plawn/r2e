//! `#[ignore]` on a `#[case]` breaks suite teardown accounting in both
//! directions, so the macro rejects it instead of half-working.

#[derive(Default)]
struct IgnoredCaseSuite;

#[r2e::test_suite(tracing = false)]
impl IgnoredCaseSuite {
    #[case]
    fn runs(&mut self) {}

    #[case]
    #[ignore]
    fn skipped(&mut self) {}

    #[after_all]
    fn teardown(&mut self) {}
}

fn main() {}
