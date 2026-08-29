//! A paused clock is a current-thread-only feature: the multi-thread runtime
//! panics while building, which no `.expect(...)` can name. Rejected at compile
//! time instead.

#[derive(Default)]
struct PausedSuite;

#[r2e::test_suite(tracing = false, start_paused = true)]
impl PausedSuite {
    #[case]
    fn runs(&mut self) {}
}

fn main() {}
