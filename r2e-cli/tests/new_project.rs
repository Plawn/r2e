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
    assert!(cargo.contains("dev-reload = [\"r2e/dev-reload\"]"));
}

#[test]
#[serial]
fn new_creates_thin_main_rs() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    new_project::run("myapp", default_opts()).unwrap();

    // app_main! owns the include, runtime entry point, and launch path.
    let main = fs::read_to_string("myapp/src/main.rs").unwrap();
    assert!(main.contains("r2e::app_main!(Myapp);"));
    assert!(main.contains("recursion_limit"));
    assert!(!main.contains("cfg"));
    assert!(!main.contains("include!"));
    assert!(!main.contains(".build_state()"));
    assert!(!main.contains("register_controller"));
}

#[test]
#[serial]
fn new_creates_shared_app_source_and_library_wrapper() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    new_project::run("myapp", default_opts()).unwrap();

    let app = fs::read_to_string("myapp/src/app.rs").unwrap();
    assert!(app.contains("pub struct Myapp;"));
    assert!(app.contains("impl App for Myapp"));
    assert!(app.contains("async fn build(b: AppBuilder"));
    // New DI model: state is inferred via `.build_state().await`, no typed state.
    assert!(app.contains(".build_state()"));
    assert!(!app.contains("build_state!"));
    assert!(!app.contains("AppState"));
    assert!(app.contains(".register_controller::<HelloController>()"));
    // recursion_limit belongs in the crate roots, not the included app source.
    assert!(!app.contains("recursion_limit"));

    let lib = fs::read_to_string("myapp/src/lib.rs").unwrap();
    assert!(lib.contains("include!(\"app.rs\")"));
    assert!(lib.contains("recursion_limit"));

    let env = fs::read_to_string("myapp/src/env.rs").unwrap();
    assert!(env.contains("pub struct AppEnv"));
    assert!(env.contains("setup_env"));
}

#[test]
#[serial]
fn new_creates_blueprint_test() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    new_project::run("myapp", default_opts()).unwrap();

    // The integration test boots the real app.
    let test = fs::read_to_string("myapp/tests/app.rs").unwrap();
    assert!(test.contains("#[r2e::test(app = myapp::Myapp)]"));
    assert!(test.contains("TestApp"));

    let cargo = fs::read_to_string("myapp/Cargo.toml").unwrap();
    assert!(cargo.contains("[dev-dependencies]"));
    assert!(cargo.contains("r2e-test"));
}

#[test]
#[serial]
fn new_creates_agent_docs() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    new_project::run("myapp", default_opts()).unwrap();

    let agents = fs::read_to_string("myapp/AGENTS.md").unwrap();
    assert!(agents.contains("App trait"));
    assert!(agents.contains("Do X, not Y"));
    assert!(agents.contains("#[r2e::test(app = myapp::Myapp)]"));

    // CLAUDE.md imports AGENTS.md — single source of truth.
    let claude = fs::read_to_string("myapp/CLAUDE.md").unwrap();
    assert_eq!(claude.trim(), "@AGENTS.md");
}

#[test]
#[serial]
fn new_hyphenated_name_uses_crate_ident() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    new_project::run("my-app", default_opts()).unwrap();

    // main.rs no longer needs the crate ident; tests still do.
    let main = fs::read_to_string("my-app/src/main.rs").unwrap();
    assert!(main.contains("r2e::app_main!(MyApp);"));
    let test = fs::read_to_string("my-app/tests/app.rs").unwrap();
    assert!(test.contains("#[r2e::test(app = my_app::MyApp)]"));
}

#[test]
#[serial]
fn new_does_not_create_state_rs() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    new_project::run("myapp", default_opts()).unwrap();

    // The typed-state path was removed: no state.rs is generated.
    assert!(!Path::new("myapp/src/state.rs").exists());
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

    // Pool is produced from config in the canonical app source.
    assert!(!Path::new("myapp/src/state.rs").exists());
    let app = fs::read_to_string("myapp/src/app.rs").unwrap();
    assert!(app.contains("SqlitePool"));
    assert!(app.contains("#[producer]"));
    assert!(app.contains(".register::<CreatePool>()"));

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

    let app = fs::read_to_string("myapp/src/app.rs").unwrap();
    assert!(app.contains("PgPool"));
}

#[test]
#[serial]
fn new_with_db_postgres_alias() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    let mut opts = default_opts();
    opts.db = Some("pg".to_string());
    new_project::run("myapp", opts).unwrap();

    let app = fs::read_to_string("myapp/src/app.rs").unwrap();
    assert!(app.contains("PgPool"));
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

    // JWT validator is produced from config in the canonical app source.
    assert!(!Path::new("myapp/src/state.rs").exists());
    let app = fs::read_to_string("myapp/src/app.rs").unwrap();
    assert!(app.contains("JwtClaimsValidator"));
    assert!(app.contains(".register::<JwtValidator>()"));

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
    assert!(
        cargo.contains("schemars"),
        "Expected schemars dependency when openapi is enabled"
    );

    let app = fs::read_to_string("myapp/src/app.rs").unwrap();
    assert!(app.contains("OpenApiPlugin"));
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
    assert!(cargo.contains("r2e-grpc-build"));

    assert!(Path::new("myapp/proto/greeter.proto").exists());
    assert!(Path::new("myapp/build.rs").exists());

    let app = fs::read_to_string("myapp/src/app.rs").unwrap();
    assert!(app.contains("GrpcServer"));

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

    // No typed state — all wiring lives in the canonical app source.
    assert!(!Path::new("myapp/src/state.rs").exists());
    let app = fs::read_to_string("myapp/src/app.rs").unwrap();
    assert!(app.contains("SqlitePool"));
    assert!(app.contains("LocalEventBus"));
    assert!(app.contains("JwtClaimsValidator"));
    assert!(app.contains("Scheduler"));
    assert!(app.contains("Executor"));
    assert!(app.contains("OpenApiPlugin"));
    assert!(app.contains("GrpcServer"));
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
