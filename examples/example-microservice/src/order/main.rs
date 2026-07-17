//! Order Service binary entry point.
//!
//! Two `[[bin]]` targets share one crate, so each uses `launch!` (rather than
//! `app_main!`, which assumes a single `src/app.rs`) to run its own [`App`].

#[r2e::main]
async fn main() {
    r2e::launch!(example_microservice::order::OrderApp)
        .await
        .unwrap();
}
