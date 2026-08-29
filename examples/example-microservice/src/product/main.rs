//! Product Service binary entry point.
//!
//! Two `[[bin]]` targets share one crate, so each uses `launch!` (rather than
//! `app_main!`, which assumes a single `src/app.rs`) to run its own [`App`].

#[r2e::main]
async fn main() {
    r2e::exit_on_boot_error(r2e::launch!(example_microservice::product::ProductApp).await);
}
