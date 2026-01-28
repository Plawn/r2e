use colored::Colorize;
use std::path::Path;

const KNOWN_EXTENSIONS: &[(&str, &str)] = &[
    ("security", "quarlus-security"),
    ("data", "quarlus-data"),
    ("openapi", "quarlus-openapi"),
    ("events", "quarlus-events"),
    ("scheduler", "quarlus-scheduler"),
    ("cache", "quarlus-cache"),
    ("rate-limit", "quarlus-rate-limit"),
    ("utils", "quarlus-utils"),
    ("prometheus", "quarlus-prometheus"),
    ("test", "quarlus-test"),
];

pub fn run(extension: &str) -> Result<(), Box<dyn std::error::Error>> {
    let cargo_path = Path::new("Cargo.toml");
    if !cargo_path.exists() {
        return Err("No Cargo.toml found in current directory. Are you in a Quarlus project?".into());
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

    std::fs::write(cargo_path, doc.to_string())?;

    println!(
        "{} Added {} to Cargo.toml dependencies",
        "✓".green(),
        crate_name.cyan()
    );
    println!("  Run `cargo build` to fetch the new dependency.");

    Ok(())
}
