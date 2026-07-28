//! `#[r2e::test_suite]` supports shared state, hooks, unordered cases, and
//! optional ordered cases.

#[derive(Default)]
struct MySuite {
    count: usize,
}

#[r2e::test_suite(tracing = false)]
impl MySuite {
    #[before_each]
    fn reset(&mut self) {
        self.count += 1;
    }

    #[case]
    async fn unordered_case(&mut self) {
        assert!(self.count >= 1);
    }

    #[case(order = 10)]
    fn ordered_case(&mut self) -> Result<(), &'static str> {
        assert!(self.count >= 1);
        Ok(())
    }
}

fn main() {}
