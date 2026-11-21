//! A suite boots its app only for `#[before_all]`'s `TestApp` binding; without
//! one the declared resources would never start.
struct MyApp;
struct Db;
struct Suite;

#[r2e::test_suite(app = MyApp, resources(Db))]
impl Suite {
    #[before_all]
    async fn boot() -> Self {
        Suite
    }

    #[case]
    async fn runs(&mut self) {}
}

fn main() {}
