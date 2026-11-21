//! `resources(...)` are started for the app the test boots and wired into its
//! builder, so they only mean something next to `app = ...`.
struct Db;

#[r2e::test(resources(Db))]
async fn talks_to_a_database() {}

fn main() {}
