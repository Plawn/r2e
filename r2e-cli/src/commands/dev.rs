use colored::Colorize;
use std::process::Command;

pub fn run(open_browser: bool) -> Result<(), Box<dyn std::error::Error>> {
    // Check if cargo-watch is installed
    let has_watch = Command::new("cargo")
        .args(["watch", "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !has_watch {
        eprintln!(
            "{} cargo-watch is not installed. Install it with:",
            "!".yellow()
        );
        eprintln!("  cargo install cargo-watch");
        return Err("cargo-watch not found".into());
    }

    println!("{}", "Starting R2E dev server...".blue().bold());
    println!();

    // Show routes before starting
    if super::routes::run().is_ok() {
        println!();
    }

    // Build the cargo-watch command
    let mut cmd = Command::new("cargo");
    cmd.arg("watch")
        .arg("-w")
        .arg("src")
        .arg("-w")
        .arg("application.yaml")
        .arg("-w")
        .arg("application-dev.yaml")
        .arg("-w")
        .arg("migrations")
        .arg("--ignore")
        .arg("target/")
        .arg("-x")
        .arg("run");

    // Set R2E_PROFILE=dev
    cmd.env("R2E_PROFILE", "dev");

    println!(
        "{} Watching src/, application*.yaml, migrations/",
        "->".blue()
    );
    println!("{} Press {} to stop", "->".blue(), "Ctrl+C".yellow());
    println!();

    // Optionally open browser after a delay
    if open_browser {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let _ = open::that("http://localhost:8080");
        });
    }

    let status = cmd.status()?;

    if !status.success() {
        return Err("cargo-watch exited with error".into());
    }

    Ok(())
}
