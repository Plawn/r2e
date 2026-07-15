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
/// Checks 9 aspects of the current directory:
/// 1. `Cargo.toml` exists (Error if missing)
/// 2. R2E dependency in Cargo.toml (Error if missing)
/// 3. `application.yaml` exists (Warning if missing)
/// 4. `src/controllers/` exists and has `.rs` files (Warning if missing)
/// 5. Rust toolchain (`rustc --version`) (Error if missing)
/// 6. `dx` (Dioxus CLI) installed (Warning if missing)
/// 7. `migrations/` exists when data features are used (Warning if missing)
/// 8. `src/main.rs` contains `.serve()` call (Warning if missing)
/// 9. Bean registration count vs. recursion limit (Warning if over ~120
///    registrations and the crate root lacks `#![recursion_limit]`)
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

    // 6. Check dx (Dioxus CLI) for hot-reload
    check(
        "Dioxus CLI (for r2e dev)",
        || match Command::new("dx").arg("--version").output() {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                CheckResult::Ok(version)
            }
            _ => CheckResult::Warning("Not installed. Run: cargo install dioxus-cli".into()),
        },
        &mut issues,
    );

    // 7. Check migrations directory if data feature is used
    check(
        "Migrations directory",
        || {
            let content = std::fs::read_to_string("Cargo.toml").unwrap_or_default();
            let uses_database = content.contains("r2e-data-sqlx")
                || content.contains("r2e-data-diesel")
                || content.contains("sqlx-sqlite")
                || content.contains("sqlx-postgres")
                || content.contains("sqlx-mysql")
                || content.contains("diesel-sqlite")
                || content.contains("diesel-postgres")
                || content.contains("diesel-mysql");
            if uses_database {
                if Path::new("migrations").exists() {
                    let count = std::fs::read_dir("migrations")
                        .map(|dir| dir.count())
                        .unwrap_or(0);
                    CheckResult::Ok(format!("{} migration files", count))
                } else {
                    CheckResult::Warning(
                        "Database integration used but no migrations/ directory".into(),
                    )
                }
            } else {
                CheckResult::Ok("Database integration not used (skipped)".into())
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

    // 9. Recursion limit heuristic.
    //
    // Each `.provide(` / `.register::<` / `.register(` builds a type-level
    // index-witness chain. Past ~127 provisions the chain exceeds rustc's
    // default recursion limit of 128, so the crate root needs
    // `#![recursion_limit = "512"]`. A macro cannot inject a crate-level
    // attribute, so we warn about it here.
    check(
        "Bean registration count",
        || {
            const THRESHOLD: usize = 120;
            let count = count_di_registrations(Path::new("src"));

            if count <= THRESHOLD {
                return CheckResult::Ok("bean count within default recursion limit".into());
            }

            // Look for an actual (uncommented) crate-level attribute — the
            // scaffold ships a commented-out hint that must not count.
            let root_has_limit = ["src/main.rs", "src/lib.rs"].iter().any(|p| {
                std::fs::read_to_string(p)
                    .map(|c| {
                        c.lines()
                            .any(|line| line.trim_start().starts_with("#![recursion_limit"))
                    })
                    .unwrap_or(false)
            });

            if root_has_limit {
                CheckResult::Ok(format!(
                    "{count} registrations; recursion_limit already set"
                ))
            } else {
                CheckResult::Warning(format!(
                    "{count} bean registrations exceed the default recursion limit — \
                     add #![recursion_limit = \"512\"] to src/main.rs (or src/lib.rs)"
                ))
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

/// Count dependency-injection registration calls across all `.rs` files
/// under `dir` (recursively).
///
/// Counts occurrences of `.provide(`, `.register::<`, and `.register(`.
/// Cheap and heuristic — it is a plain substring scan, not a parser.
fn count_di_registrations(dir: &Path) -> usize {
    let mut count = 0;

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return 0,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            count += count_di_registrations(&path);
        } else if path.extension().map(|ext| ext == "rs").unwrap_or(false) {
            if let Ok(content) = std::fs::read_to_string(&path) {
                count += content.matches(".provide(").count();
                count += content.matches(".register::<").count();
                count += content.matches(".register(").count();
            }
        }
    }

    count
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
