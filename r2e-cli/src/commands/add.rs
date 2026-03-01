use colored::Colorize;
use std::path::Path;

const KNOWN_EXTENSIONS: &[(&str, &str)] = &[
    ("security", "r2e-security"),
    ("data", "r2e-data"),
    ("data-sqlx", "r2e-data-sqlx"),
    ("data-diesel", "r2e-data-diesel"),
    ("openapi", "r2e-openapi"),
    ("events", "r2e-events"),
    ("scheduler", "r2e-scheduler"),
    ("cache", "r2e-cache"),
    ("rate-limit", "r2e-rate-limit"),
    ("utils", "r2e-utils"),
    ("prometheus", "r2e-prometheus"),
    ("grpc", "r2e-grpc"),
    ("test", "r2e-test"),
];

/// Add an R2E extension crate to the project's `Cargo.toml`.
///
/// Looks up `extension` in the known extensions map, parses `Cargo.toml`
/// with `toml_edit`, and inserts the dependency with version `"0.1"`.
///
/// Returns an error if:
/// - `Cargo.toml` does not exist
/// - The extension name is unknown
///
/// Prints a warning (but returns `Ok`) if the dependency is already present.
pub fn run(extension: &str) -> Result<(), Box<dyn std::error::Error>> {
    let cargo_path = Path::new("Cargo.toml");
    if !cargo_path.exists() {
        return Err("No Cargo.toml found in current directory. Are you in a R2E project?".into());
    }

    let (_, crate_name) = KNOWN_EXTENSIONS
        .iter()
        .find(|(name, _)| *name == extension)
        .ok_or_else(|| {
            let available: Vec<_> = KNOWN_EXTENSIONS.iter().map(|(n, _)| *n).collect();
            format!(
                "Unknown extension '{}'. Available: {}",
                extension,
                available.join(", ")
            )
        })?;

    let content = std::fs::read_to_string(cargo_path)?;
    let mut doc = content.parse::<toml_edit::DocumentMut>()?;

    let deps = doc
        .entry("dependencies")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or("dependencies is not a table")?;

    if deps.contains_key(crate_name) {
        println!(
            "{} Extension '{}' is already in Cargo.toml",
            "!".yellow(),
            extension.cyan()
        );
        return Ok(());
    }

    // Add the dependency as a simple version string
    deps.insert(crate_name, toml_edit::value(format!("0.1")));

    // Add companion dependencies for extensions that require them
    if extension == "openapi" && !deps.contains_key("schemars") {
        deps.insert("schemars", toml_edit::value("1"));
    }

    std::fs::write(cargo_path, doc.to_string())?;

    println!(
        "{} Added {} to Cargo.toml dependencies",
        "✓".green(),
        crate_name.cyan()
    );
    if extension == "openapi" {
        println!(
            "{} Also added {} (required for #[derive(JsonSchema)])",
            "✓".green(),
            "schemars".cyan()
        );
    }
    println!("  Run `cargo build` to fetch the new dependency.");

    Ok(())
}
