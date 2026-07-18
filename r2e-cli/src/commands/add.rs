use colored::Colorize;
use std::path::Path;

const KNOWN_EXTENSIONS: &[(&str, &str)] = &[
    ("security", "r2e-security"),
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

    // gRPC is a full scaffold (deps + build.rs + proto/ + service skeleton),
    // not just a dependency insert.
    if extension == "grpc" {
        return scaffold_grpc(cargo_path);
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
    deps.insert(crate_name, toml_edit::value("0.1"));

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

/// `r2e add grpc` — full gRPC setup: enable the `grpc`/`grpc-reflection`
/// features on the `r2e` facade dependency (or fall back to a direct
/// `r2e-grpc` dependency), add the tonic/prost dependencies the generated
/// code needs, add the `r2e-grpc-build` build-dependency, and drop a
/// one-line `build.rs`, a sample `proto/greeter.proto`, and a matching
/// `src/grpc.rs` service skeleton so the project compiles a real service
/// immediately.
fn scaffold_grpc(cargo_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(cargo_path)?;
    let mut doc = content.parse::<toml_edit::DocumentMut>()?;

    let package_name = doc
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("app")
        .to_string();

    // Mirror the source of the `r2e` dependency for r2e-grpc-build (git
    // checkouts of unpublished R2E must not mix registry versions in),
    // including any branch/rev/tag pin.
    let r2e_git_source: Vec<(&str, String)> = ["git", "branch", "rev", "tag"]
        .iter()
        .filter_map(|key| {
            let value = doc
                .get("dependencies")?
                .get("r2e")?
                .get(key)?
                .as_str()?
                .to_string();
            Some((*key, value))
        })
        .collect();

    let deps = doc
        .entry("dependencies")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or("dependencies is not a table")?;

    if let Some(r2e_dep) = deps.get_mut("r2e") {
        for feature in ["grpc", "grpc-reflection"] {
            if add_dep_feature(r2e_dep, feature)? {
                println!(
                    "{} Enabled feature {} on the {} dependency",
                    "✓".green(),
                    feature.cyan(),
                    "r2e".cyan()
                );
            }
        }
    } else if !deps.contains_key("r2e-grpc") {
        deps.insert("r2e-grpc", toml_edit::value("0.1"));
        println!(
            "{} Added {} to Cargo.toml dependencies",
            "✓".green(),
            "r2e-grpc".cyan()
        );
    }

    // Generated proto code references `::tonic`, `::tonic_prost`, `::prost`.
    for (name, version) in [
        ("tonic", "~0.14"),
        ("tonic-prost", "~0.14"),
        ("prost", "~0.14"),
    ] {
        if !deps.contains_key(name) {
            deps.insert(name, toml_edit::value(version));
            println!("{} Added {} {}", "✓".green(), name.cyan(), version);
        }
    }

    let build_deps = doc
        .entry("build-dependencies")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or("build-dependencies is not a table")?;
    if !build_deps.contains_key("r2e-grpc-build") {
        let item = if r2e_git_source.is_empty() {
            toml_edit::value("0.1")
        } else {
            let mut t = toml_edit::InlineTable::new();
            for (key, value) in &r2e_git_source {
                t.insert(*key, value.as_str().into());
            }
            toml_edit::value(t)
        };
        build_deps.insert("r2e-grpc-build", item);
        println!(
            "{} Added {} to build-dependencies",
            "✓".green(),
            "r2e-grpc-build".cyan()
        );
    }

    std::fs::write(cargo_path, doc.to_string())?;

    // build.rs — one line; never overwrite an existing build script.
    let build_rs = Path::new("build.rs");
    if !build_rs.exists() {
        std::fs::write(build_rs, super::templates::project::build_rs())?;
        println!("{} Created {}", "✓".green(), "build.rs".cyan());
    } else if !std::fs::read_to_string(build_rs)?.contains("r2e_grpc_build") {
        println!(
            "{} build.rs already exists — add `r2e_grpc_build::compile()?;` to it yourself",
            "!".yellow()
        );
    }

    // Sample proto + service skeleton — only on a blank slate, so we never
    // fight protos or a grpc module the project already has.
    let proto_dir = Path::new("proto");
    let has_protos = proto_dir.exists()
        && std::fs::read_dir(proto_dir)?
            .filter_map(Result::ok)
            .any(|e| e.path().extension().is_some_and(|ext| ext == "proto"));
    if !has_protos {
        std::fs::create_dir_all(proto_dir)?;
        std::fs::write(
            proto_dir.join("greeter.proto"),
            super::templates::project::greeter_proto(&package_name),
        )?;
        println!("{} Created {}", "✓".green(), "proto/greeter.proto".cyan());

        // Directory layout (`src/grpc/mod.rs` + one file per service) — the
        // same one `r2e generate grpc-service` extends.
        let grpc_dir = Path::new("src/grpc");
        if Path::new("src").exists() && !grpc_dir.exists() && !Path::new("src/grpc.rs").exists() {
            std::fs::create_dir_all(grpc_dir)?;
            std::fs::write(
                grpc_dir.join("mod.rs"),
                super::templates::project::grpc_mod_rs(),
            )?;
            std::fs::write(
                grpc_dir.join("greeter.rs"),
                super::templates::project::grpc_greeter_rs(&package_name),
            )?;
            println!(
                "{} Created {}",
                "✓".green(),
                "src/grpc/ (mod.rs + greeter.rs)".cyan()
            );
        }
    }

    println!();
    println!("Wire it into your App (src/app.rs):");
    println!();
    println!("  use r2e::r2e_grpc::{{AppBuilderGrpcExt, GrpcServer}};");
    println!("  pub mod grpc;");
    println!("  use grpc::GreeterService;");
    println!();
    println!("  b.plugin(GrpcServer::on_port(\"0.0.0.0:50051\").with_reflection())");
    println!("      // …");
    println!("      .build_state().await");
    println!("      .register_grpc_service::<GreeterService>()");
    println!();
    println!("  Then: cargo build   # drop more .proto files in proto/ anytime");

    Ok(())
}

/// Add `feature` to a dependency item's `features` array, converting a bare
/// version string (`r2e = "0.1"`) into an inline table first. Returns true
/// if the feature was added, false if it was already present.
fn add_dep_feature(
    item: &mut toml_edit::Item,
    feature: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    // Normalize `r2e = "0.1"` to `r2e = { version = "0.1" }`.
    if let Some(version) = item.as_str().map(str::to_string) {
        let mut t = toml_edit::InlineTable::new();
        t.insert("version", version.into());
        *item = toml_edit::value(t);
    }

    let features = match item {
        toml_edit::Item::Value(toml_edit::Value::InlineTable(t)) => t
            .entry("features")
            .or_insert_with(|| toml_edit::Value::Array(toml_edit::Array::new()))
            .as_array_mut()
            .ok_or("dependency `features` is not an array")?,
        toml_edit::Item::Table(t) => t
            .entry("features")
            .or_insert(toml_edit::Item::Value(toml_edit::Value::Array(
                toml_edit::Array::new(),
            )))
            .as_array_mut()
            .ok_or("dependency `features` is not an array")?,
        _ => return Err("unsupported r2e dependency shape in Cargo.toml".into()),
    };

    if features.iter().any(|f| f.as_str() == Some(feature)) {
        return Ok(false);
    }
    features.push(feature);
    Ok(true)
}
