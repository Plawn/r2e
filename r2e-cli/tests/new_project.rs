use r2e_cli::commands::new_project::{self, CliNewOpts};
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

fn default_opts() -> CliNewOpts {
    CliNewOpts {
        db: None,
        auth: false,
        openapi: false,
        metrics: false,
        grpc: false,
        full: false,
        no_interactive: true,
    }
}

// ── Basic project creation ──────────────────────────────────────────

#[test]
#[serial]
fn new_creates_project_dir() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    new_project::run("myapp", default_opts()).unwrap();

    assert!(Path::new("myapp").is_dir());
}

#[test]
#[serial]
fn new_creates_cargo_toml() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    new_project::run("myapp", default_opts()).unwrap();

    let cargo = fs::read_to_string("myapp/Cargo.toml").unwrap();
    assert!(cargo.contains("name = \"myapp\""));
    assert!(cargo.contains("r2e"));
    assert!(cargo.contains("tokio"));
}

#[test]
#[serial]
fn new_creates_main_rs() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    new_project::run("myapp", default_opts()).unwrap();

    let main = fs::read_to_string("myapp/src/main.rs").unwrap();
    assert!(main.contains("#[r2e::main]"));
    assert!(main.contains("serve("));
    assert!(main.contains("AppBuilder"));
}

#[test]
#[serial]
fn new_creates_state_rs() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    new_project::run("myapp", default_opts()).unwrap();

    let state = fs::read_to_string("myapp/src/state.rs").unwrap();
    assert!(state.contains("pub struct AppState"));
    assert!(state.contains("BeanState"));
}

#[test]
#[serial]
fn new_creates_hello_controller() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    new_project::run("myapp", default_opts()).unwrap();

    assert!(Path::new("myapp/src/controllers/hello.rs").exists());
    let hello = fs::read_to_string("myapp/src/controllers/hello.rs").unwrap();
    assert!(hello.contains("HelloController"));

    let mod_rs = fs::read_to_string("myapp/src/controllers/mod.rs").unwrap();
    assert!(mod_rs.contains("pub mod hello;"));
}

#[test]
#[serial]
fn new_creates_application_yaml() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    new_project::run("myapp", default_opts()).unwrap();

    let yaml = fs::read_to_string("myapp/application.yaml").unwrap();
    assert!(yaml.contains("myapp"));
}

#[test]
#[serial]
fn new_creates_gitignore() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    new_project::run("myapp", default_opts()).unwrap();

    let gitignore = fs::read_to_string("myapp/.gitignore").unwrap();
    assert!(gitignore.contains("/target"));
}

// ── Database options ────────────────────────────────────────────────

#[test]
#[serial]
fn new_with_db_sqlite() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    let mut opts = default_opts();
    opts.db = Some("sqlite".to_string());
    new_project::run("myapp", opts).unwrap();

    let cargo = fs::read_to_string("myapp/Cargo.toml").unwrap();
    assert!(cargo.contains("sqlx"));
    assert!(cargo.contains("sqlite"));

    // migrations/ directory should be created
    assert!(Path::new("myapp/migrations").is_dir());

    let state = fs::read_to_string("myapp/src/state.rs").unwrap();
    assert!(state.contains("SqlitePool"));

    let yaml = fs::read_to_string("myapp/application.yaml").unwrap();
    assert!(yaml.contains("database:"));
    assert!(yaml.contains("sqlite:"));
}

#[test]
#[serial]
fn new_with_db_postgres() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    let mut opts = default_opts();
    opts.db = Some("postgres".to_string());
    new_project::run("myapp", opts).unwrap();

    let cargo = fs::read_to_string("myapp/Cargo.toml").unwrap();
    assert!(cargo.contains("sqlx"));
    assert!(cargo.contains("postgres"));

    let state = fs::read_to_string("myapp/src/state.rs").unwrap();
    assert!(state.contains("PgPool"));
}

#[test]
#[serial]
fn new_with_db_postgres_alias() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    let mut opts = default_opts();
    opts.db = Some("pg".to_string());
    new_project::run("myapp", opts).unwrap();

    let state = fs::read_to_string("myapp/src/state.rs").unwrap();
    assert!(state.contains("PgPool"));
}

// ── Feature flags ───────────────────────────────────────────────────

#[test]
#[serial]
fn new_with_auth() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    let mut opts = default_opts();
    opts.auth = true;
    new_project::run("myapp", opts).unwrap();

    let cargo = fs::read_to_string("myapp/Cargo.toml").unwrap();
    assert!(cargo.contains("security"));

    let state = fs::read_to_string("myapp/src/state.rs").unwrap();
    assert!(state.contains("JwtClaimsValidator"));

    let yaml = fs::read_to_string("myapp/application.yaml").unwrap();
    assert!(yaml.contains("security:"));
    assert!(yaml.contains("jwt:"));
}

#[test]
#[serial]
fn new_with_openapi() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    let mut opts = default_opts();
    opts.openapi = true;
    new_project::run("myapp", opts).unwrap();

    let cargo = fs::read_to_string("myapp/Cargo.toml").unwrap();
    assert!(cargo.contains("openapi"));
    assert!(cargo.contains("schemars"), "Expected schemars dependency when openapi is enabled");

    let main = fs::read_to_string("myapp/src/main.rs").unwrap();
    assert!(main.contains("OpenApiPlugin"));
}

#[test]
#[serial]
fn new_with_grpc() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    let mut opts = default_opts();
    opts.grpc = true;
    new_project::run("myapp", opts).unwrap();

    let cargo = fs::read_to_string("myapp/Cargo.toml").unwrap();
    assert!(cargo.contains("tonic"));
    assert!(cargo.contains("prost"));
    assert!(cargo.contains("[build-dependencies]"));
    assert!(cargo.contains("tonic-build"));

    assert!(Path::new("myapp/proto/greeter.proto").exists());
    assert!(Path::new("myapp/build.rs").exists());

    let main = fs::read_to_string("myapp/src/main.rs").unwrap();
    assert!(main.contains("GrpcServer"));

    let yaml = fs::read_to_string("myapp/application.yaml").unwrap();
    assert!(yaml.contains("grpc:"));
}

#[test]
#[serial]
fn new_full() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    let mut opts = default_opts();
    opts.full = true;
    new_project::run("myapp", opts).unwrap();

    let cargo = fs::read_to_string("myapp/Cargo.toml").unwrap();
    assert!(cargo.contains("security"));
    assert!(cargo.contains("openapi"));
    assert!(cargo.contains("events"));
    assert!(cargo.contains("scheduler"));
    assert!(cargo.contains("data"));
    assert!(cargo.contains("grpc"));
    assert!(cargo.contains("sqlx"));
    assert!(cargo.contains("tonic"));

    assert!(Path::new("myapp/migrations").is_dir());
    assert!(Path::new("myapp/proto/greeter.proto").exists());
    assert!(Path::new("myapp/build.rs").exists());

    let state = fs::read_to_string("myapp/src/state.rs").unwrap();
    assert!(state.contains("SqlitePool"));
    assert!(state.contains("LocalEventBus"));
    assert!(state.contains("JwtClaimsValidator"));

    let main = fs::read_to_string("myapp/src/main.rs").unwrap();
    assert!(main.contains("Scheduler"));
    assert!(main.contains("OpenApiPlugin"));
    assert!(main.contains("GrpcServer"));
}

#[test]
#[serial]
fn new_no_interactive_uses_defaults() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    // no_interactive with no flags → minimal project
    new_project::run("myapp", default_opts()).unwrap();

    let cargo = fs::read_to_string("myapp/Cargo.toml").unwrap();
    // Should not contain any optional features
    assert!(!cargo.contains("sqlx"));
    assert!(!cargo.contains("security"));

    // No migrations dir
    assert!(!Path::new("myapp/migrations").exists());
}

#[test]
#[serial]
fn new_already_exists_errors() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    fs::create_dir("myapp").unwrap();

    let result = new_project::run("myapp", default_opts());
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already exists"));
}
