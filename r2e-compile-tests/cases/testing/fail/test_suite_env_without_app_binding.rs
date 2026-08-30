//! Same, one step further: there *is* a `#[before_all]`, but it binds nothing
//! from the booted app, so the suite never boots one and `env = ...` is dead
//! (task #988).
struct MyApp;

struct BadSuite;

#[r2e::test_suite(app = MyApp, env = shared_env())]
impl BadSuite {
    #[before_all]
    async fn setup() -> Self {
        Self
    }

    #[case]
    async fn a_case(&mut self) {}
}

fn main() {}
