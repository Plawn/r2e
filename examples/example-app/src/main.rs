// Subsecond patches the binary tip crate. Compile the same source used by the
// library directly into this crate in dev so controller/service edits are hot.
#[cfg(feature = "dev-reload")]
include!("app.rs");

#[cfg(not(feature = "dev-reload"))]
use example_app::ExampleApp;

#[r2e::main]
async fn main() {
    r2e::launch!(ExampleApp).await.unwrap();
}
