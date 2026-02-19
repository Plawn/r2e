use r2e_cli::commands::generate::{self, parse_fields, rust_type_to_sql};
use serial_test::serial;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

// ── CWD Guard ───────────────────────────────────────────────────────

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

// ════════════════════════════════════════════════════════════════════
// Phase 2: Field Parsing
// ════════════════════════════════════════════════════════════════════

#[test]
fn parse_field_string() {
    let fields = parse_fields(&["name:String".into()]).unwrap();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "name");
    assert_eq!(fields[0].rust_type, "String");
    assert!(!fields[0].is_optional);
}

#[test]
fn parse_field_i64() {
    let fields = parse_fields(&["age:i64".into()]).unwrap();
    assert_eq!(fields[0].name, "age");
    assert_eq!(fields[0].rust_type, "i64");
    assert!(!fields[0].is_optional);
}

#[test]
fn parse_field_bool() {
    let fields = parse_fields(&["active:bool".into()]).unwrap();
    assert_eq!(fields[0].name, "active");
    assert_eq!(fields[0].rust_type, "bool");
    assert!(!fields[0].is_optional);
}

#[test]
fn parse_field_optional() {
    let fields = parse_fields(&["email:Option<String>".into()]).unwrap();
    assert_eq!(fields[0].name, "email");
    assert_eq!(fields[0].rust_type, "Option<String>");
    assert!(fields[0].is_optional);
}

#[test]
fn parse_multiple_fields() {
    let fields = parse_fields(&["name:String".into(), "age:i64".into()]).unwrap();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name, "name");
    assert_eq!(fields[1].name, "age");
}

#[test]
fn parse_field_invalid_format() {
    let result = parse_fields(&["name".into()]);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Invalid field format"));
}

#[test]
fn parse_field_empty_input() {
    let fields = parse_fields(&[]).unwrap();
    assert!(fields.is_empty());
}

// ════════════════════════════════════════════════════════════════════
// Phase 2: SQL Type Mapping
// ════════════════════════════════════════════════════════════════════

#[test]
fn sql_type_string() {
    assert_eq!(rust_type_to_sql("String"), "TEXT");
}

#[test]
fn sql_type_str() {
    assert_eq!(rust_type_to_sql("&str"), "TEXT");
}

#[test]
fn sql_type_i32() {
    assert_eq!(rust_type_to_sql("i32"), "INTEGER");
}

#[test]
fn sql_type_i64() {
    assert_eq!(rust_type_to_sql("i64"), "INTEGER");
}

#[test]
fn sql_type_u64() {
    assert_eq!(rust_type_to_sql("u64"), "INTEGER");
}

#[test]
fn sql_type_f32() {
    assert_eq!(rust_type_to_sql("f32"), "REAL");
}

#[test]
fn sql_type_f64() {
    assert_eq!(rust_type_to_sql("f64"), "REAL");
}

#[test]
fn sql_type_bool() {
    assert_eq!(rust_type_to_sql("bool"), "BOOLEAN");
}

#[test]
fn sql_type_option_string() {
    assert_eq!(rust_type_to_sql("Option<String>"), "TEXT");
}

#[test]
fn sql_type_option_i64() {
    assert_eq!(rust_type_to_sql("Option<i64>"), "INTEGER");
}

#[test]
fn sql_type_unknown_defaults_to_text() {
    assert_eq!(rust_type_to_sql("CustomType"), "TEXT");
}

// ════════════════════════════════════════════════════════════════════
// Phase 3: Controller Generation
// ════════════════════════════════════════════════════════════════════

#[test]
#[serial]
fn generate_controller_creates_file() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("src/controllers").unwrap();

    generate::controller("UserController").unwrap();

    assert!(Path::new("src/controllers/user_controller.rs").exists());
}

#[test]
#[serial]
fn generate_controller_valid_content() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("src/controllers").unwrap();

    generate::controller("UserController").unwrap();

    let content = fs::read_to_string("src/controllers/user_controller.rs").unwrap();
    assert!(content.contains("#[derive(Controller)]"));
    assert!(content.contains("pub struct UserController"));
    assert!(content.contains("#[routes]"));
    assert!(content.contains("impl UserController"));
}

#[test]
#[serial]
fn generate_controller_updates_mod_rs() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("src/controllers").unwrap();
    fs::write("src/controllers/mod.rs", "pub mod hello;\n").unwrap();

    generate::controller("UserController").unwrap();

    let mod_content = fs::read_to_string("src/controllers/mod.rs").unwrap();
    assert!(mod_content.contains("pub mod user_controller;"));
    assert!(mod_content.contains("pub mod hello;"));
}

#[test]
#[serial]
fn generate_controller_no_mod_rs_no_error() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("src/controllers").unwrap();
    // No mod.rs exists

    generate::controller("UserController").unwrap();

    assert!(Path::new("src/controllers/user_controller.rs").exists());
    // mod.rs should NOT be created when it didn't exist
    assert!(!Path::new("src/controllers/mod.rs").exists());
}

#[test]
#[serial]
fn generate_controller_already_exists_errors() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("src/controllers").unwrap();
    fs::write("src/controllers/user_controller.rs", "existing").unwrap();

    let result = generate::controller("UserController");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already exists"));
}

// ════════════════════════════════════════════════════════════════════
// Phase 3: Service Generation
// ════════════════════════════════════════════════════════════════════

#[test]
#[serial]
fn generate_service_creates_file() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("src").unwrap();

    generate::service("UserService").unwrap();

    assert!(Path::new("src/user_service.rs").exists());
}

#[test]
#[serial]
fn generate_service_valid_content() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("src").unwrap();

    generate::service("UserService").unwrap();

    let content = fs::read_to_string("src/user_service.rs").unwrap();
    assert!(content.contains("#[derive(Clone)]"));
    assert!(content.contains("pub struct UserService"));
    assert!(content.contains("pub fn new()"));
}

#[test]
#[serial]
fn generate_service_already_exists_errors() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("src").unwrap();
    fs::write("src/user_service.rs", "existing").unwrap();

    let result = generate::service("UserService");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already exists"));
}

// ════════════════════════════════════════════════════════════════════
// Phase 3: CRUD Generation
// ════════════════════════════════════════════════════════════════════

#[test]
#[serial]
fn generate_crud_creates_all_files() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("migrations").unwrap();

    generate::crud("User", &["name:String".into(), "email:String".into()]).unwrap();

    assert!(Path::new("src/models/user.rs").exists());
    assert!(Path::new("src/services/user_service.rs").exists());
    assert!(Path::new("src/controllers/user_controller.rs").exists());
    assert!(Path::new("tests/user_test.rs").exists());
    // Migration file should exist (starts with timestamp)
    let migration_files: Vec<_> = fs::read_dir("migrations")
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(migration_files.len(), 1);
}

#[test]
#[serial]
fn generate_crud_model_has_fields() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    generate::crud("User", &["name:String".into(), "age:i64".into()]).unwrap();

    let model = fs::read_to_string("src/models/user.rs").unwrap();
    assert!(model.contains("pub struct User"));
    assert!(model.contains("pub id: i64"));
    assert!(model.contains("pub name: String"));
    assert!(model.contains("pub age: i64"));
    // Create/Update request should NOT have id
    assert!(model.contains("pub struct CreateUserRequest"));
    assert!(model.contains("pub struct UpdateUserRequest"));
}

#[test]
#[serial]
fn generate_crud_controller_has_endpoints() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    generate::crud("User", &["name:String".into()]).unwrap();

    let controller = fs::read_to_string("src/controllers/user_controller.rs").unwrap();
    assert!(controller.contains("#[get(\"/\")]"));
    assert!(controller.contains("#[post(\"/\")]"));
    assert!(controller.contains("#[put("));
    assert!(controller.contains("#[delete("));
    assert!(controller.contains("#[controller(path = \"/users\""));
}

#[test]
#[serial]
fn generate_crud_migration_sql() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("migrations").unwrap();

    generate::crud(
        "User",
        &[
            "name:String".into(),
            "age:i64".into(),
            "active:bool".into(),
        ],
    )
    .unwrap();

    let migration_file = fs::read_dir("migrations")
        .unwrap()
        .filter_map(|e| e.ok())
        .next()
        .unwrap();
    let sql = fs::read_to_string(migration_file.path()).unwrap();
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS users"));
    assert!(sql.contains("name TEXT NOT NULL"));
    assert!(sql.contains("age INTEGER NOT NULL"));
    assert!(sql.contains("active BOOLEAN NOT NULL"));
    assert!(sql.contains("id INTEGER PRIMARY KEY AUTOINCREMENT"));
}

#[test]
#[serial]
fn generate_crud_migration_timestamp_in_filename() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("migrations").unwrap();

    generate::crud("User", &["name:String".into()]).unwrap();

    let filename = fs::read_dir("migrations")
        .unwrap()
        .filter_map(|e| e.ok())
        .next()
        .unwrap()
        .file_name()
        .to_string_lossy()
        .to_string();

    // Filename should be <14-digit-timestamp>_create_users.sql
    assert!(filename.ends_with("_create_users.sql"));
    let timestamp_part = &filename[..14];
    assert!(timestamp_part.chars().all(|c| c.is_ascii_digit()));
}

#[test]
#[serial]
fn generate_crud_no_migration_without_dir() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    // No migrations/ directory

    generate::crud("User", &["name:String".into()]).unwrap();

    assert!(!Path::new("migrations").exists());
}

#[test]
#[serial]
fn generate_crud_optional_field_nullable_in_migration() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("migrations").unwrap();

    generate::crud("User", &["bio:Option<String>".into()]).unwrap();

    let migration_file = fs::read_dir("migrations")
        .unwrap()
        .filter_map(|e| e.ok())
        .next()
        .unwrap();
    let sql = fs::read_to_string(migration_file.path()).unwrap();
    // Optional fields should NOT have NOT NULL
    assert!(sql.contains("bio TEXT"));
    assert!(!sql.contains("bio TEXT NOT NULL"));
}

#[test]
#[serial]
fn generate_crud_updates_mod_rs() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    generate::crud("User", &["name:String".into()]).unwrap();

    let models_mod = fs::read_to_string("src/models/mod.rs").unwrap();
    assert!(models_mod.contains("pub mod user;"));

    let services_mod = fs::read_to_string("src/services/mod.rs").unwrap();
    assert!(services_mod.contains("pub mod user_service;"));

    let controllers_mod = fs::read_to_string("src/controllers/mod.rs").unwrap();
    assert!(controllers_mod.contains("pub mod user_controller;"));
}

#[test]
#[serial]
fn generate_crud_test_file() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    generate::crud("User", &["name:String".into()]).unwrap();

    let test_content = fs::read_to_string("tests/user_test.rs").unwrap();
    assert!(test_content.contains("test_list_users"));
    assert!(test_content.contains("test_create_user"));
    assert!(test_content.contains("test_get_user_not_found"));
    assert!(test_content.contains("test_delete_user"));
}

// ════════════════════════════════════════════════════════════════════
// Phase 3: Middleware Generation
// ════════════════════════════════════════════════════════════════════

#[test]
#[serial]
fn generate_middleware_creates_file() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    generate::middleware("AuditLog").unwrap();

    assert!(Path::new("src/middleware/audit_log.rs").exists());
}

#[test]
#[serial]
fn generate_middleware_has_interceptor() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    generate::middleware("AuditLog").unwrap();

    let content = fs::read_to_string("src/middleware/audit_log.rs").unwrap();
    assert!(content.contains("pub struct AuditLog"));
    assert!(content.contains("Interceptor<R, S>"));
    assert!(content.contains("fn around"));
}

#[test]
#[serial]
fn generate_middleware_updates_mod_rs() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    generate::middleware("AuditLog").unwrap();

    let mod_content = fs::read_to_string("src/middleware/mod.rs").unwrap();
    assert!(mod_content.contains("pub mod audit_log;"));
}

#[test]
#[serial]
fn generate_middleware_already_exists_errors() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("src/middleware").unwrap();
    fs::write("src/middleware/audit_log.rs", "existing").unwrap();

    let result = generate::middleware("AuditLog");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already exists"));
}

// ════════════════════════════════════════════════════════════════════
// Phase 3: gRPC Service Generation
// ════════════════════════════════════════════════════════════════════

#[test]
#[serial]
fn generate_grpc_creates_proto_and_service() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    generate::grpc_service("User", "myapp").unwrap();

    assert!(Path::new("proto/user.proto").exists());
    assert!(Path::new("src/grpc/user.rs").exists());
}

#[test]
#[serial]
fn generate_grpc_proto_content() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    generate::grpc_service("User", "myapp").unwrap();

    let proto = fs::read_to_string("proto/user.proto").unwrap();
    assert!(proto.contains("package myapp;"));
    assert!(proto.contains("service User"));
    assert!(proto.contains("rpc GetUser"));
    assert!(proto.contains("rpc ListUser"));
}

#[test]
#[serial]
fn generate_grpc_service_content() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    generate::grpc_service("User", "myapp").unwrap();

    let service = fs::read_to_string("src/grpc/user.rs").unwrap();
    assert!(service.contains("pub struct UserService"));
    assert!(service.contains("#[grpc_routes("));
    assert!(service.contains("async fn get_user"));
    assert!(service.contains("async fn list_user"));
}

#[test]
#[serial]
fn generate_grpc_already_exists_errors() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("proto").unwrap();
    fs::write("proto/user.proto", "existing").unwrap();

    let result = generate::grpc_service("User", "myapp");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already exists"));
}
