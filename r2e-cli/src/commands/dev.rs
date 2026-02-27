use colored::Colorize;
use std::process::Command;

pub fn run(port: Option<u16>, extra_features: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Check dx is installed
    ensure_dx_installed()?;

    // 2. Ensure Dioxus.toml exists
    ensure_dioxus_config()?;

    // 3. Build the dx command
    let mut cmd = Command::new("dx");
    cmd.args(["serve", "--hot-patch"]);

    // Merge features: always include dev-reload + any user features
    let mut features = vec!["dev-reload".to_string()];
    features.extend(extra_features);
    cmd.args(["--features", &features.join(",")]);

    // Set R2E_PROFILE=dev
    cmd.env("R2E_PROFILE", "dev");

    // Forward port as env var
    if let Some(port) = port {
        cmd.env("R2E_PORT", port.to_string());
    }

    // 4. Run
    println!(
        "{}",
        "Starting R2E dev server with Subsecond hot-reload...".blue().bold()
    );
    println!(
        "{} Changes to handler code will be applied in <1s",
        "->".blue()
    );
    println!("{} Press {} to stop", "->".blue(), "Ctrl+C".yellow());
    println!();

    let status = cmd.status()?;

    if !status.success() {
        return Err(format!(
            "dx serve exited with code {}",
            status.code().unwrap_or(-1)
        )
        .into());
    }

    Ok(())
}

fn ensure_dx_installed() -> Result<(), Box<dyn std::error::Error>> {
    match Command::new("dx").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!(
                "{} dioxus-cli found: {}",
                "ok".green(),
                version.trim()
            );
            Ok(())
        }
        _ => {
            eprintln!(
                "{} dioxus-cli (dx) is not installed. Install it with:",
                "!".yellow()
            );
            eprintln!("  cargo install dioxus-cli");
            eprintln!();
            eprintln!("Or via the install script:");
            eprintln!("  curl -sSL https://dioxus.dev/install.sh | bash");
            Err("dioxus-cli not found".into())
        }
    }
}

fn ensure_dioxus_config() -> Result<(), Box<dyn std::error::Error>> {
    let config_path = "Dioxus.toml";
    if !std::path::Path::new(config_path).exists() {
        // Try to read the project name from Cargo.toml
        let project_name = std::fs::read_to_string("Cargo.toml")
            .ok()
            .and_then(|content| {
                content
                    .parse::<toml_edit::DocumentMut>()
                    .ok()
                    .and_then(|doc| {
                        doc.get("package")
                            .and_then(|p| p.get("name"))
                            .and_then(|n| n.as_str().map(String::from))
                    })
            })
            .unwrap_or_else(|| "r2e-app".to_string());

        println!(
            "{} Creating minimal Dioxus.toml for hot-reload...",
            "->".blue()
        );
        std::fs::write(
            config_path,
            format!(
                r#"[application]
name = "{project_name}"

[application.tools]
"#
            ),
        )?;
    }
    Ok(())
}
