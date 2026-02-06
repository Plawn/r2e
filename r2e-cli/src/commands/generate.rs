use colored::Colorize;
use std::fs;
use std::path::Path;

pub fn controller(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let file_name = to_snake_case(name);
    let path = Path::new("src/controllers").join(format!("{file_name}.rs"));

    if path.exists() {
        return Err(format!("Controller file '{}' already exists", path.display()).into());
    }

    if !Path::new("src/controllers").exists() {
        fs::create_dir_all("src/controllers")?;
    }

    let content = format!(
        r#"use axum::Json;
use r2e_core::prelude::*;
use serde::{{Deserialize, Serialize}};

// TODO: import your state type
// use crate::state::AppState;

#[derive(Controller)]
#[controller(state = AppState)]
pub struct {name} {{
    // #[inject]
    // your_service: YourService,
}}

#[routes]
impl {name} {{
    #[get("/your-path")]
    async fn list(&self) -> Json<String> {{
        Json("Hello from {name}".into())
    }}
}}
"#
    );

    fs::write(&path, content)?;

    println!(
        "{} Generated controller: {}",
        "✓".green(),
        path.display().to_string().cyan()
    );

    // Update mod.rs
    let mod_path = Path::new("src/controllers/mod.rs");
    if mod_path.exists() {
        let existing = fs::read_to_string(mod_path)?;
        let mod_line = format!("pub mod {file_name};\n");
        if !existing.contains(&mod_line) {
            fs::write(mod_path, format!("{existing}{mod_line}"))?;
            println!("{} Updated src/controllers/mod.rs", "✓".green());
        }
    }

    Ok(())
}

pub fn service(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let file_name = to_snake_case(name);
    let path = Path::new("src").join(format!("{file_name}.rs"));

    if path.exists() {
        return Err(format!("Service file '{}' already exists", path.display()).into());
    }

    let content = format!(
        r#"use std::sync::Arc;

#[derive(Clone)]
pub struct {name} {{
    // Add your dependencies here
}}

impl {name} {{
    pub fn new() -> Self {{
        Self {{}}
    }}

    // Add your methods here
}}
"#
    );

    fs::write(&path, content)?;

    println!(
        "{} Generated service: {}",
        "✓".green(),
        path.display().to_string().cyan()
    );

    Ok(())
}

fn to_snake_case(name: &str) -> String {
    let mut result = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(c.to_lowercase().next().unwrap());
        } else {
            result.push(c);
        }
    }
    result
}
