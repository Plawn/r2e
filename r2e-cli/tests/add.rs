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
fn add_data() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::write("Cargo.toml", minimal_cargo_toml()).unwrap();

    add::run("data").unwrap();

    let cargo = fs::read_to_string("Cargo.toml").unwrap();
    assert!(cargo.contains("r2e-data"));
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
    assert!(cargo.contains("schemars"), "Expected schemars companion dependency");
}

#[test]
#[serial]
fn add_all_known_extensions() {
    let known = [
        "security",
        "data",
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
    assert!(cargo.contains("schemars"), "Expected schemars companion dependency for openapi");
}
