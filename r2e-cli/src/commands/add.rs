use colored::Colorize;
use std::path::Path;

/// Version requirement written for every R2E dependency `r2e add` inserts.
/// Keep in step with `[workspace.package] version` at the repo root.
const R2E_DEP_VERSION: &str = "0.3";

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
    ("mcp", "r2e-mcp"),
    ("test", "r2e-test"),
];

/// Add an R2E extension crate to the project's `Cargo.toml`.
///
/// Looks up `extension` in the known extensions map and updates `Cargo.toml`
/// with `toml_edit`. Most extensions insert a dependency with version
/// [`R2E_DEP_VERSION`]; MCP and gRPC have facade-aware setup paths.
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
    if extension == "mcp" {
        return add_mcp(cargo_path);
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
    deps.insert(crate_name, toml_edit::value(R2E_DEP_VERSION));

    // Add companion dependencies for extensions that require them
    if matches!(extension, "openapi" | "mcp") && !deps.contains_key("schemars") {
        deps.insert("schemars", toml_edit::value("1"));
    }

    std::fs::write(cargo_path, doc.to_string())?;

    println!(
        "{} Added {} to Cargo.toml dependencies",
        "✓".green(),
        crate_name.cyan()
    );
    if matches!(extension, "openapi" | "mcp") {
        println!(
            "{} Also added {} (required for #[derive(JsonSchema)])",
            "✓".green(),
            "schemars".cyan()
        );
    }
    println!("  Run `cargo build` to fetch the new dependency.");

    Ok(())
}

/// `r2e add mcp` — dependencies plus an idempotent MCP bean/config scaffold.
fn add_mcp(cargo_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(cargo_path)?;
    let mut doc = content.parse::<toml_edit::DocumentMut>()?;

    let package_name = doc
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml_edit::Item::as_str)
        .unwrap_or("app")
        .to_string();
    let deps = doc
        .entry("dependencies")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or("dependencies is not a table")?;

    let uses_facade = deps.contains_key("r2e");
    if let Some(r2e_dep) = deps.get_mut("r2e") {
        if add_dep_feature(r2e_dep, "mcp")? {
            println!(
                "{} Enabled feature {} on the {} dependency",
                "✓".green(),
                "mcp".cyan(),
                "r2e".cyan()
            );
        } else {
            println!(
                "{} Feature {} is already enabled on the {} dependency",
                "!".yellow(),
                "mcp".cyan(),
                "r2e".cyan()
            );
        }
    } else if !deps.contains_key("r2e-mcp") {
        deps.insert("r2e-mcp", toml_edit::value(R2E_DEP_VERSION));
        println!(
            "{} Added {} to Cargo.toml dependencies",
            "✓".green(),
            "r2e-mcp".cyan()
        );
    } else {
        println!(
            "{} Extension '{}' is already in Cargo.toml",
            "!".yellow(),
            "mcp".cyan()
        );
    }

    if !uses_facade && !deps.contains_key("r2e-core") {
        deps.insert("r2e-core", toml_edit::value(R2E_DEP_VERSION));
        println!(
            "{} Added {} for the direct-crate scaffold",
            "✓".green(),
            "r2e-core".cyan()
        );
    }

    if !deps.contains_key("schemars") {
        deps.insert("schemars", toml_edit::value("1"));
        println!(
            "{} Also added {} (required for #[derive(JsonSchema)])",
            "✓".green(),
            "schemars".cyan()
        );
    }

    if !deps.contains_key("serde") {
        let mut serde = toml_edit::InlineTable::new();
        serde.insert("version", "1".into());
        let mut features = toml_edit::Array::new();
        features.push("derive");
        serde.insert("features", features.into());
        deps.insert("serde", toml_edit::value(serde));
        println!(
            "{} Also added {} with derive support",
            "✓".green(),
            "serde".cyan()
        );
    } else if add_dep_feature(deps.get_mut("serde").expect("checked above"), "derive")? {
        println!(
            "{} Enabled feature {} on the {} dependency",
            "✓".green(),
            "derive".cyan(),
            "serde".cyan()
        );
    }

    std::fs::write(cargo_path, doc.to_string())?;

    let src = Path::new("src");
    let mcp_rs = src.join("mcp.rs");
    let has_mcp_module = mcp_rs.exists() || src.join("mcp/mod.rs").exists();
    if src.exists() && !has_mcp_module {
        std::fs::write(
            &mcp_rs,
            super::templates::project::mcp_service_rs(uses_facade),
        )?;
        println!("{} Created {}", "✓".green(), "src/mcp.rs".cyan());
        if let Some(root) = declare_module(src, "mcp")? {
            println!(
                "{} Declared {} in {}",
                "✓".green(),
                "mod mcp;".cyan(),
                root.display().to_string().cyan()
            );
        }
    } else if has_mcp_module {
        println!(
            "{} MCP source module already exists — left it unchanged",
            "!".yellow()
        );
    }

    let config_path = Path::new("application.yaml");
    let mut config = if config_path.exists() {
        std::fs::read_to_string(config_path)?
    } else {
        String::new()
    };
    let has_mcp_config = config
        .lines()
        .any(|line| !line.starts_with(char::is_whitespace) && line.trim_end() == "mcp:");
    if !has_mcp_config {
        if !config.is_empty() && !config.ends_with('\n') {
            config.push('\n');
        }
        if !config.is_empty() {
            config.push('\n');
        }
        config.push_str(&format!("mcp:\n  path: /mcp\n  name: {package_name}\n"));
        std::fs::write(config_path, config)?;
        println!("{} Configured {}", "✓".green(), "application.yaml".cyan());
    }

    println!();
    println!("Wire the generated MCP adapter into your App (src/app.rs):");
    println!();
    println!("  use crate::mcp::McpTools;");
    println!();
    println!("  b.plugin(McpServer::new())");
    println!("      .build_state().await");
    println!("      .register_mcp_service::<McpTools>()");
    println!();
    println!("  Then: cargo build");
    Ok(())
}

/// Declare `mod <name>;` in the crate root (`src/lib.rs`, else `src/main.rs`)
/// so a freshly scaffolded `src/<name>.rs` compiles without a manual edit.
/// Returns the root that was edited, `None` when there is no root or the
/// module is already declared (`mod x;` / `pub mod x;`, possibly attributed).
fn declare_module(
    src: &Path,
    name: &str,
) -> Result<Option<std::path::PathBuf>, Box<dyn std::error::Error>> {
    let Some(root) = ["lib.rs", "main.rs"]
        .into_iter()
        .map(|file| src.join(file))
        .find(|path| path.exists())
    else {
        return Ok(None);
    };
    let content = std::fs::read_to_string(&root)?;
    let declares = |line: &str| {
        let line = line.trim();
        let line = line.strip_prefix("pub ").unwrap_or(line);
        line.strip_prefix("mod ")
            .and_then(|rest| rest.strip_suffix(';'))
            .is_some_and(|declared| declared.trim() == name)
    };
    if content.lines().any(declares) {
        return Ok(None);
    }
    let declaration = format!("mod {name};\n");
    // Group with the existing module declarations when there are any (right
    // after the last one), otherwise lead the file (after inner attributes).
    let lines: Vec<&str> = content.lines().collect();
    let after_last_mod = lines
        .iter()
        .rposition(|line| {
            let line = line.trim();
            let line = line.strip_prefix("pub ").unwrap_or(line);
            line.starts_with("mod ") && line.ends_with(';')
        })
        .map(|index| index + 1);
    let insert_at = after_last_mod.unwrap_or_else(|| {
        lines
            .iter()
            .take_while(|line| line.trim_start().starts_with("#!"))
            .count()
    });
    let mut updated = String::with_capacity(content.len() + declaration.len());
    for (index, line) in lines.iter().enumerate() {
        if index == insert_at {
            updated.push_str(&declaration);
        }
        updated.push_str(line);
        updated.push('\n');
    }
    if insert_at >= lines.len() {
        updated.push_str(&declaration);
    }
    std::fs::write(&root, updated)?;
    Ok(Some(root))
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
        deps.insert("r2e-grpc", toml_edit::value(R2E_DEP_VERSION));
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
            toml_edit::value(R2E_DEP_VERSION)
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
/// version string (`r2e = "0.3"`) into an inline table first. Returns true
/// if the feature was added, false if it was already present.
fn add_dep_feature(
    item: &mut toml_edit::Item,
    feature: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    // Normalize `r2e = "0.3"` to `r2e = { version = "0.3" }`.
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
