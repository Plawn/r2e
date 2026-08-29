//! `worker_threads = 0` makes the runtime builder panic in its setter.

#[derive(Default)]
struct ZeroWorkerSuite;

#[r2e::test_suite(tracing = false, worker_threads = 0)]
impl ZeroWorkerSuite {
    #[case]
    fn runs(&mut self) {}
}

fn main() {}
