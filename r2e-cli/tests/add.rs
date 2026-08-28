use r2e_cli::commands::add;
use serial_test::serial;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct CwdGuard {
    original: PathBuf,
}

impl CwdGuard {
    fn new(path: &Path) -> Self {
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(path).unwrap();
        CwdGuard { original }
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
    }
}

fn minimal_cargo_toml() -> &'static str {
    "[package]\nname = \"test-app\"\nversion = \"0.1.0\"\n\n[dependencies]\nr2e = \"0.1\"\n"
}

#[test]
#[serial]
fn add_security() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::write("Cargo.toml", minimal_cargo_toml()).unwrap();

    add::run("security").unwrap();

    let cargo = fs::read_to_string("Cargo.toml").unwrap();
    assert!(cargo.contains("r2e-security"));
}

#[test]
#[serial]
fn add_events() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::write("Cargo.toml", minimal_cargo_toml()).unwrap();

    add::run("events").unwrap();

    let cargo = fs::read_to_string("Cargo.toml").unwrap();
    assert!(cargo.contains("r2e-events"));
}

#[test]
#[serial]
fn add_unknown_extension_errors() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::write("Cargo.toml", minimal_cargo_toml()).unwrap();

    let result = add::run("unknown-thing");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Unknown extension"));
    assert!(err.contains("Available:"));
}

#[test]
#[serial]
fn add_already_present_no_duplicate() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::write(
        "Cargo.toml",
        "[package]\nname = \"test-app\"\nversion = \"0.1.0\"\n\n[dependencies]\nr2e = \"0.1\"\nr2e-security = \"0.1\"\n",
    )
    .unwrap();

    // Should succeed without error (prints warning)
    add::run("security").unwrap();

    // Should not duplicate the entry
    let cargo = fs::read_to_string("Cargo.toml").unwrap();
    let count = cargo.matches("r2e-security").count();
    assert_eq!(count, 1);
}

#[test]
#[serial]
fn add_no_cargo_toml_errors() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    // No Cargo.toml

    let result = add::run("security");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("No Cargo.toml"));
}

#[test]
#[serial]
fn add_multiple_extensions() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::write("Cargo.toml", minimal_cargo_toml()).unwrap();

    add::run("security").unwrap();
    add::run("events").unwrap();
    add::run("cache").unwrap();

    let cargo = fs::read_to_string("Cargo.toml").unwrap();
    assert!(cargo.contains("r2e-security"));
    assert!(cargo.contains("r2e-events"));
    assert!(cargo.contains("r2e-cache"));
}

#[test]
#[serial]
fn add_openapi_includes_schemars() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::write("Cargo.toml", minimal_cargo_toml()).unwrap();

    add::run("openapi").unwrap();

    let cargo = fs::read_to_string("Cargo.toml").unwrap();
    assert!(cargo.contains("r2e-openapi"));
    assert!(
        cargo.contains("schemars"),
        "Expected schemars companion dependency"
    );
}

#[test]
#[serial]
fn add_mcp_enables_facade_feature_and_schemars() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::write("Cargo.toml", minimal_cargo_toml()).unwrap();

    add::run("mcp").unwrap();

    let cargo = fs::read_to_string("Cargo.toml").unwrap();
    assert!(cargo.contains("r2e = { version = \"0.1\", features = [\"mcp\"] }"));
    assert!(cargo.contains("schemars = \"1\""));
    assert!(cargo.contains("serde = { version = \"1\", features = [\"derive\"] }"));
    assert!(!cargo.contains("r2e-mcp"));
    assert!(fs::read_to_string("application.yaml")
        .unwrap()
        .contains("mcp:\n  path: /mcp"));
}

#[test]
#[serial]
fn add_mcp_preserves_facade_features_and_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::write(
        "Cargo.toml",
        "[package]\nname = \"test-app\"\nversion = \"0.1.0\"\n\n[dependencies]\nr2e = { version = \"0.1\", features = [\"security\"] }\n",
    )
    .unwrap();

    add::run("mcp").unwrap();
    add::run("mcp").unwrap();

    let cargo = fs::read_to_string("Cargo.toml").unwrap();
    assert_eq!(cargo.matches("\"mcp\"").count(), 1);
    assert!(cargo.contains("\"security\""));
    assert_eq!(cargo.matches("schemars").count(), 1);
    assert_eq!(cargo.matches("serde").count(), 1);
    assert_eq!(
        fs::read_to_string("application.yaml")
            .unwrap()
            .matches("mcp:")
            .count(),
        1
    );
}

#[test]
#[serial]
fn add_mcp_scaffolds_source_without_overwriting_it() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::write("Cargo.toml", minimal_cargo_toml()).unwrap();
    fs::create_dir("src").unwrap();

    add::run("mcp").unwrap();
    let source = fs::read_to_string("src/mcp.rs").unwrap();
    assert!(source.contains("#[derive(Debug, Deserialize, JsonSchema, ObjectParams)]"));
    assert!(source.contains("#[controller]"));
    assert!(source.contains("#[mcp_routes]"));
    assert!(source.contains("#[tool(read_only)]"));
    assert!(source.contains("pub struct McpTools"));

    fs::write("src/mcp.rs", "// user-owned\n").unwrap();
    add::run("mcp").unwrap();
    assert_eq!(fs::read_to_string("src/mcp.rs").unwrap(), "// user-owned\n");
}

#[test]
#[serial]
fn add_mcp_repairs_missing_schemars_for_direct_dependency() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::write(
        "Cargo.toml",
        "[package]\nname = \"test-app\"\nversion = \"0.1.0\"\n\n[dependencies]\nr2e-mcp = \"0.3\"\n",
    )
    .unwrap();

    add::run("mcp").unwrap();

    let cargo = fs::read_to_string("Cargo.toml").unwrap();
    assert_eq!(cargo.matches("r2e-mcp").count(), 1);
    assert!(cargo.contains("schemars = \"1\""));
}

#[test]
#[serial]
fn add_mcp_adds_direct_dependency_without_facade() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::write(
        "Cargo.toml",
        "[package]\nname = \"test-app\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
    )
    .unwrap();

    add::run("mcp").unwrap();

    let cargo = fs::read_to_string("Cargo.toml").unwrap();
    assert!(cargo.contains("r2e-mcp = \"0.3\""));
    assert!(cargo.contains("r2e-core = \"0.3\""));
    assert!(cargo.contains("schemars = \"1\""));
}

#[test]
#[serial]
fn add_mcp_direct_scaffold_uses_direct_crates() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::write(
        "Cargo.toml",
        "[package]\nname = \"direct-app\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
    )
    .unwrap();
    fs::create_dir("src").unwrap();

    add::run("mcp").unwrap();

    let source = fs::read_to_string("src/mcp.rs").unwrap();
    assert!(source.contains("use r2e_core::prelude::{controller, mcp_routes, tool, ObjectParams};"));
    assert!(source.contains("use r2e_mcp::Params;"));
    assert!(!source.contains("use r2e::"));
    assert!(fs::read_to_string("application.yaml")
        .unwrap()
        .contains("name: direct-app"));
}

#[test]
#[serial]
fn add_all_known_extensions() {
    let known = [
        "security",
        "data-sqlx",
        "data-diesel",
        "openapi",
        "events",
        "scheduler",
        "cache",
        "rate-limit",
        "utils",
        "prometheus",
        "grpc",
        "test",
    ];

    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::write("Cargo.toml", minimal_cargo_toml()).unwrap();

    for ext in &known {
        add::run(ext).unwrap();
    }

    let cargo = fs::read_to_string("Cargo.toml").unwrap();
    for ext in &known {
        let crate_name = format!("r2e-{}", ext);
        assert!(
            cargo.contains(&crate_name),
            "Expected {} in Cargo.toml",
            crate_name
        );
    }
    // openapi should also add schemars as a companion dependency
    assert!(
        cargo.contains("schemars"),
        "Expected schemars companion dependency for openapi"
    );
}
