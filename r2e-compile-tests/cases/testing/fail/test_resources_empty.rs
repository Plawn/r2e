//! An empty `resources()` starts nothing — a leftover, not a declaration.
struct MyApp;

#[r2e::test(app = MyApp, resources())]
async fn talks_to_a_database(app: r2e::r2e_test::TestApp) {}

fn main() {}
