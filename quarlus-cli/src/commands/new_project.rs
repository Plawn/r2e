use colored::Colorize;
use std::fs;
use std::path::Path;

pub fn run(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let project_dir = Path::new(name);
    if project_dir.exists() {
        return Err(format!("Directory '{}' already exists", name).into());
    }

    println!("{} Creating new Quarlus project: {}", "→".blue(), name.green());

    fs::create_dir_all(project_dir.join("src/controllers"))?;
    fs::create_dir_all(project_dir.join("src"))?;

    // Cargo.toml
    let cargo_toml = format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[dependencies]
quarlus-core = "0.1"
quarlus-macros = "0.1"
quarlus-security = "0.1"
axum = "0.8"
tokio = {{ version = "1", features = ["full"] }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
tracing = "0.1"
tracing-subscriber = {{ version = "0.3", features = ["env-filter"] }}
"#
    );
    fs::write(project_dir.join("Cargo.toml"), cargo_toml)?;

    // main.rs
    let main_rs = r#"use quarlus_core::AppBuilder;

mod controllers;

#[tokio::main]
async fn main() {
    quarlus_core::init_tracing();

    // TODO: set up your state and controllers

    tracing::info!("Starting application");
}
"#;
    fs::write(project_dir.join("src/main.rs"), main_rs)?;

    // controllers/mod.rs
    fs::write(
        project_dir.join("src/controllers/mod.rs"),
        "// Add your controllers here\n",
    )?;

    // application.yaml
    let app_yaml = format!(
        r#"app:
  name: {name}
  port: 3000
"#
    );
    fs::write(project_dir.join("application.yaml"), app_yaml)?;

    println!("{} Project '{}' created successfully!", "✓".green(), name.green());
    println!();
    println!("  cd {name}");
    println!("  cargo run");

    Ok(())
}
