use r2e_cli::commands::doctor;
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

#[test]
#[serial]
fn doctor_empty_directory() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    // Should not error even with nothing present
    let result = doctor::run();
    assert!(result.is_ok());
}

#[test]
#[serial]
fn doctor_missing_cargo_toml() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    // No Cargo.toml → run succeeds (prints errors internally)
    assert!(doctor::run().is_ok());
}

#[test]
#[serial]
fn doctor_missing_r2e_dep() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    fs::write(
        "Cargo.toml",
        "[package]\nname = \"test\"\n[dependencies]\naxum = \"0.7\"\n",
    )
    .unwrap();

    // No r2e dep → run succeeds (prints warning internally)
    assert!(doctor::run().is_ok());
}

#[test]
#[serial]
fn doctor_valid_project() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    // Set up a valid R2E project structure
    fs::write(
        "Cargo.toml",
        "[package]\nname = \"test\"\n[dependencies]\nr2e = \"0.1\"\n",
    )
    .unwrap();
    fs::write("application.yaml", "app:\n  name: test\n").unwrap();
    fs::create_dir_all("src/controllers").unwrap();
    fs::write("src/controllers/hello.rs", "").unwrap();
    fs::write(
        "src/main.rs",
        "fn main() { builder.serve(\"0.0.0.0:8080\"); }",
    )
    .unwrap();

    assert!(doctor::run().is_ok());
}

#[test]
#[serial]
fn doctor_missing_config() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    fs::write(
        "Cargo.toml",
        "[package]\nname = \"test\"\n[dependencies]\nr2e = \"0.1\"\n",
    )
    .unwrap();
    // No application.yaml → warning, but still Ok

    assert!(doctor::run().is_ok());
}

#[test]
#[serial]
fn doctor_missing_controllers_dir() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    fs::write(
        "Cargo.toml",
        "[package]\nname = \"test\"\n[dependencies]\nr2e = \"0.1\"\n",
    )
    .unwrap();
    fs::write("application.yaml", "app:\n  name: test\n").unwrap();
    // No src/controllers/ → warning

    assert!(doctor::run().is_ok());
}

#[test]
#[serial]
fn doctor_data_feature_without_migrations() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    fs::write(
        "Cargo.toml",
        "[package]\nname = \"test\"\n[dependencies]\nr2e = \"0.1\"\nr2e-data = \"0.1\"\n",
    )
    .unwrap();
    // r2e-data dep but no migrations/ → warning

    assert!(doctor::run().is_ok());
}

#[test]
#[serial]
fn doctor_missing_serve_call() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    fs::write(
        "Cargo.toml",
        "[package]\nname = \"test\"\n[dependencies]\nr2e = \"0.1\"\n",
    )
    .unwrap();
    fs::create_dir_all("src").unwrap();
    fs::write("src/main.rs", "fn main() { println!(\"hello\"); }").unwrap();
    // main.rs without .serve() → warning

    assert!(doctor::run().is_ok());
}
