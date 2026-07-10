use example_app::{app_with_env, AppEnv};
use r2e::prelude::*;

// --- Setup: runs once, persists across hot-patches ---

async fn setup() -> AppEnv {
    // Print a test JWT for curl usage
    println!("=== Test JWT (valid 1h) ===");
    println!("{}", example_app::demo_token());
    println!();

    example_app::setup().await
}

// --- Server: hot-patched on every code change when dev-reload is enabled ---

#[r2e::main]
async fn main(env: AppEnv) {
    app_with_env(AppBuilder::new(), env)
        .await
        .serve("0.0.0.0:3001")
        .await
        .inspect_err(|e| eprintln!("=== SERVE ERROR: {e} ==="))
        .unwrap();
}
