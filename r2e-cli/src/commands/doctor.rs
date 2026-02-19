use colored::Colorize;
use std::path::Path;
use std::process::Command;

enum CheckResult {
    Ok(String),
    Warning(String),
    Error(String),
}

/// Run project health diagnostics.
///
/// Checks 8 aspects of the current directory:
/// 1. `Cargo.toml` exists (Error if missing)
/// 2. R2E dependency in Cargo.toml (Error if missing)
/// 3. `application.yaml` exists (Warning if missing)
/// 4. `src/controllers/` exists and has `.rs` files (Warning if missing)
/// 5. Rust toolchain (`rustc --version`) (Error if missing)
/// 6. `cargo-watch` installed (Warning if missing)
/// 7. `migrations/` exists when data features are used (Warning if missing)
/// 8. `src/main.rs` contains `.serve()` call (Warning if missing)
///
/// Results are printed with colored indicators. Always returns `Ok(())`.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", "R2E Doctor — Checking project health".bold());
    println!();

    let mut issues = 0;

    // 1. Check Cargo.toml exists
    check(
        "Cargo.toml exists",
        || {
            if Path::new("Cargo.toml").exists() {
                CheckResult::Ok("Found".into())
            } else {
                CheckResult::Error("Not in a Rust project directory".into())
            }
        },
        &mut issues,
    );

    // 2. Check r2e dependency
    check(
        "R2E dependency",
        || {
            let content = std::fs::read_to_string("Cargo.toml").unwrap_or_default();
            if content.contains("r2e") {
                CheckResult::Ok("Found".into())
            } else {
                CheckResult::Error("r2e not found in dependencies".into())
            }
        },
        &mut issues,
    );

    // 3. Check application.yaml
    check(
        "Configuration file",
        || {
            if Path::new("application.yaml").exists() {
                CheckResult::Ok("application.yaml found".into())
            } else {
                CheckResult::Warning("application.yaml not found (optional)".into())
            }
        },
        &mut issues,
    );

    // 4. Check src/controllers/ directory
    check(
        "Controllers directory",
        || {
            if Path::new("src/controllers").exists() {
                let count = std::fs::read_dir("src/controllers")
                    .map(|dir| {
                        dir.filter(|e| {
                            e.as_ref()
                                .map(|e| {
                                    e.path()
                                        .extension()
                                        .map(|ext| ext == "rs")
                                        .unwrap_or(false)
                                })
                                .unwrap_or(false)
                        })
                        .count()
                    })
                    .unwrap_or(0);
                CheckResult::Ok(format!("{} controller files", count))
            } else {
                CheckResult::Warning("src/controllers/ not found".into())
            }
        },
        &mut issues,
    );

    // 5. Check Rust toolchain
    check(
        "Rust toolchain",
        || match Command::new("rustc").arg("--version").output() {
            Ok(output) => {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                CheckResult::Ok(version)
            }
            Err(_) => CheckResult::Error("rustc not found".into()),
        },
        &mut issues,
    );

    // 6. Check cargo-watch
    check(
        "cargo-watch (for r2e dev)",
        || match Command::new("cargo").args(["watch", "--version"]).output() {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                CheckResult::Ok(version)
            }
            _ => CheckResult::Warning("Not installed. Run: cargo install cargo-watch".into()),
        },
        &mut issues,
    );

    // 7. Check migrations directory if data feature is used
    check(
        "Migrations directory",
        || {
            let content = std::fs::read_to_string("Cargo.toml").unwrap_or_default();
            if content.contains("r2e-data") || content.contains("\"data\"") {
                if Path::new("migrations").exists() {
                    let count = std::fs::read_dir("migrations")
                        .map(|dir| dir.count())
                        .unwrap_or(0);
                    CheckResult::Ok(format!("{} migration files", count))
                } else {
                    CheckResult::Warning(
                        "Data feature used but no migrations/ directory".into(),
                    )
                }
            } else {
                CheckResult::Ok("Data feature not used (skipped)".into())
            }
        },
        &mut issues,
    );

    // 8. Check that src/main.rs has serve()
    check(
        "Application entrypoint",
        || {
            let content = std::fs::read_to_string("src/main.rs").unwrap_or_default();
            if content.contains(".serve(") || content.contains("serve(") {
                CheckResult::Ok("serve() call found in main.rs".into())
            } else {
                CheckResult::Warning("No .serve() call found in main.rs".into())
            }
        },
        &mut issues,
    );

    println!();
    if issues == 0 {
        println!("{}", "All checks passed!".green().bold());
    } else {
        println!(
            "{}",
            format!("{} issue(s) found", issues).yellow().bold()
        );
    }

    Ok(())
}

fn check<F>(name: &str, f: F, issues: &mut usize)
where
    F: FnOnce() -> CheckResult,
{
    let result = f();
    match &result {
        CheckResult::Ok(msg) => {
            println!("  {} {} — {}", "✓".green(), name, msg.dimmed());
        }
        CheckResult::Warning(msg) => {
            println!("  {} {} — {}", "!".yellow(), name, msg.yellow());
            *issues += 1;
        }
        CheckResult::Error(msg) => {
            println!("  {} {} — {}", "x".red(), name, msg.red());
            *issues += 1;
        }
    }
}
