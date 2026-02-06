use colored::Colorize;
use std::fs;
use std::path::Path;

pub fn run(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let project_dir = Path::new(name);
    if project_dir.exists() {
        return Err(format!("Directory '{}' already exists", name).into());
    }

    println!("{} Creating new R2E project: {}", "→".blue(), name.green());

    fs::create_dir_all(project_dir.join("src/controllers"))?;

    // Cargo.toml
    let cargo_toml = format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[dependencies]
r2e-core = "0.1"
r2e-macros = "0.1"
axum = "0.8"
tokio = {{ version = "1", features = ["full"] }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
tracing = "0.1"
tracing-subscriber = {{ version = "0.3", features = ["env-filter"] }}
"#
    );
    fs::write(project_dir.join("Cargo.toml"), cargo_toml)?;

    // state.rs
    let state_rs = r#"use r2e_core::prelude::*;

#[derive(Clone, BeanState)]
pub struct AppState {}
"#;
    fs::write(project_dir.join("src/state.rs"), state_rs)?;

    // controllers/mod.rs
    let controllers_mod = "pub mod hello;\n";
    fs::write(project_dir.join("src/controllers/mod.rs"), controllers_mod)?;

    // controllers/hello.rs
    let hello_controller = r#"use crate::state::AppState;
use r2e_core::prelude::*;

#[derive(Controller)]
#[controller(path = "/", state = AppState)]
pub struct HelloController;

#[routes]
impl HelloController {
    #[get("/")]
    async fn hello(&self) -> &'static str {
        "Hello, World!"
    }
}
"#;
    fs::write(project_dir.join("src/controllers/hello.rs"), hello_controller)?;

    // main.rs
    let main_rs = r#"use r2e_core::prelude::*;
use r2e_core::plugins::{Health, Tracing};

mod controllers;
mod state;

use controllers::hello::HelloController;
use state::AppState;

#[tokio::main]
async fn main() {
    r2e_core::init_tracing();

    AppBuilder::new()
        .build_state::<AppState, _>()
        .with(Health)
        .with(Tracing)
        .register_controller::<HelloController>()
        .serve("0.0.0.0:8080")
        .await
        .unwrap();
}
"#;
    fs::write(project_dir.join("src/main.rs"), main_rs)?;

    // application.yaml
    let app_yaml = format!(
        r#"app:
  name: {name}
  port: 8080
"#
    );
    fs::write(project_dir.join("application.yaml"), app_yaml)?;

    println!("{} Project '{}' created successfully!", "✓".green(), name.green());
    println!();
    println!("  cd {name}");
    println!("  cargo run");
    println!();
    println!("Then visit: {}", "http://localhost:8080".cyan());

    Ok(())
}
