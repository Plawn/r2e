//! `#[case]` methods use shared suite state and do not bind per-test params.

#[derive(Default)]
struct BadSuite;

#[r2e::test_suite]
impl BadSuite {
    #[case]
    async fn has_param(&mut self, value: u32) {
        let _ = value;
    }
}

fn main() {}
