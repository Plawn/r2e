//! A suite's `env = ...` is evaluated by `#[before_all]`, when it binds the
//! booted `TestApp`. Without such a hook the expression would never run — a
//! silently ignored (and possibly expensive) initializer (task #988).
struct MyApp;

#[derive(Default)]
struct BadSuite;

#[r2e::test_suite(app = MyApp, env = shared_env())]
impl BadSuite {
    #[case]
    async fn a_case(&mut self) {}
}

fn main() {}
