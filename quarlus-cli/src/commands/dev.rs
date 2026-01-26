use colored::Colorize;
use std::process::Command;

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
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

    println!("{} Starting dev server with hot-reload...", "→".blue());

    let status = Command::new("cargo")
        .args(["watch", "-x", "run", "-w", "src/"])
        .status()?;

    if !status.success() {
        return Err("cargo-watch exited with error".into());
    }

    Ok(())
}
