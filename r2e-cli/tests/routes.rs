use r2e_cli::commands::routes::{
    self, extract_controller_path, extract_string_arg, find_next_fn_name, parse_routes_from_file,
};
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

// ── extract_controller_path ─────────────────────────────────────────

#[test]
fn extracts_controller_path() {
    let content = r#"
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController;
"#;
    assert_eq!(
        extract_controller_path(content),
        Some("/users".to_string())
    );
}

#[test]
fn extracts_controller_path_no_path() {
    let content = r#"
#[derive(Controller)]
#[controller(state = AppState)]
pub struct UserController;
"#;
    assert_eq!(extract_controller_path(content), None);
}

#[test]
fn extracts_controller_path_different_order() {
    let content = r#"
#[controller(state = AppState, path = "/api/users")]
"#;
    assert_eq!(
        extract_controller_path(content),
        Some("/api/users".to_string())
    );
}

// ── extract_string_arg ──────────────────────────────────────────────

#[test]
fn extracts_get_path() {
    assert_eq!(
        extract_string_arg(r#"    #[get("/")]"#, "get"),
        Some("/".to_string())
    );
}

#[test]
fn extracts_post_path() {
    assert_eq!(
        extract_string_arg(r#"    #[post("/create")]"#, "post"),
        Some("/create".to_string())
    );
}

#[test]
fn extracts_roles() {
    assert_eq!(
        extract_string_arg(r#"    #[roles("admin")]"#, "roles"),
        Some("admin".to_string())
    );
}

#[test]
fn extracts_no_match() {
    assert_eq!(extract_string_arg(r#"fn hello() {}"#, "get"), None);
}

// ── find_next_fn_name ───────────────────────────────────────────────

#[test]
fn finds_fn_name() {
    let content = r#"    #[get("/")]
    async fn list(&self) -> Json<Vec<User>> {"#;
    assert_eq!(find_next_fn_name(content, 0), Some("list".to_string()));
}

#[test]
fn finds_fn_name_with_gap() {
    let content = r#"    #[get("/")]
    #[roles("admin")]
    async fn admin_list(&self) -> Json<Vec<User>> {"#;
    assert_eq!(
        find_next_fn_name(content, 0),
        Some("admin_list".to_string())
    );
}

#[test]
fn finds_fn_name_none_when_too_far() {
    // fn is more than 5 lines away from the attribute
    let content = "#[get(\"/\")]\n\n\n\n\n\n\nasync fn far_away() {}";
    assert_eq!(find_next_fn_name(content, 0), None);
}

// ── parse_routes_from_file ──────────────────────────────────────────

#[test]
fn parses_routes_from_controller_file() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("user_controller.rs");
    fs::write(
        &file_path,
        r#"
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController;

#[routes]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<User>> {
        todo!()
    }

    #[post("/")]
    async fn create(&self) -> Json<User> {
        todo!()
    }

    #[get("/{id}")]
    async fn get_by_id(&self) -> Json<User> {
        todo!()
    }

    #[put("/{id}")]
    async fn update(&self) -> Json<User> {
        todo!()
    }

    #[delete("/{id}")]
    async fn delete(&self) -> Json<()> {
        todo!()
    }
}
"#,
    )
    .unwrap();

    let mut routes = Vec::new();
    parse_routes_from_file(&file_path, &mut routes).unwrap();

    assert_eq!(routes.len(), 5);

    let methods: Vec<&str> = routes.iter().map(|r| r.method.as_str()).collect();
    assert!(methods.contains(&"GET"));
    assert!(methods.contains(&"POST"));
    assert!(methods.contains(&"PUT"));
    assert!(methods.contains(&"DELETE"));
}

#[test]
fn parses_routes_combines_paths() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("user_controller.rs");
    fs::write(
        &file_path,
        r#"
#[controller(path = "/users", state = AppState)]
pub struct UserController;

#[routes]
impl UserController {
    #[get("/{id}")]
    async fn get_by_id(&self) {}
}
"#,
    )
    .unwrap();

    let mut routes = Vec::new();
    parse_routes_from_file(&file_path, &mut routes).unwrap();

    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].path, "/users/{id}");
}

#[test]
fn parses_routes_root_path_uses_base() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("hello.rs");
    fs::write(
        &file_path,
        r#"
#[controller(path = "/hello", state = AppState)]
pub struct HelloController;

#[routes]
impl HelloController {
    #[get("/")]
    async fn hello(&self) {}
}
"#,
    )
    .unwrap();

    let mut routes = Vec::new();
    parse_routes_from_file(&file_path, &mut routes).unwrap();

    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].path, "/hello");
}

#[test]
fn parses_routes_extracts_roles() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("admin.rs");
    fs::write(
        &file_path,
        r#"
#[controller(path = "/admin", state = AppState)]
pub struct AdminController;

#[routes]
impl AdminController {
    #[roles("admin")]
    #[get("/")]
    async fn admin_list(&self) {}
}
"#,
    )
    .unwrap();

    let mut routes = Vec::new();
    parse_routes_from_file(&file_path, &mut routes).unwrap();

    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].roles.as_deref(), Some("admin"));
}

#[test]
fn parses_routes_extracts_handler_name() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("test.rs");
    fs::write(
        &file_path,
        r#"
#[controller(path = "/test", state = AppState)]
pub struct TestController;

#[routes]
impl TestController {
    #[get("/")]
    async fn my_handler(&self) {}
}
"#,
    )
    .unwrap();

    let mut routes = Vec::new();
    parse_routes_from_file(&file_path, &mut routes).unwrap();

    assert_eq!(routes[0].handler, "my_handler");
}

#[test]
fn parses_routes_empty_file() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("empty.rs");
    fs::write(&file_path, "// no routes here\n").unwrap();

    let mut routes = Vec::new();
    parse_routes_from_file(&file_path, &mut routes).unwrap();

    assert!(routes.is_empty());
}

// ── routes::run() integration ───────────────────────────────────────

#[test]
#[serial]
fn routes_run_empty_dir() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("src/controllers").unwrap();

    assert!(routes::run().is_ok());
}

#[test]
#[serial]
fn routes_run_missing_controllers_dir() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());

    let result = routes::run();
    assert!(result.is_err());
}

#[test]
#[serial]
fn routes_run_with_controllers() {
    let tmp = TempDir::new().unwrap();
    let _cwd = CwdGuard::new(tmp.path());
    fs::create_dir_all("src/controllers").unwrap();
    fs::write(
        "src/controllers/user.rs",
        r#"
#[controller(path = "/users", state = AppState)]
pub struct UserController;

#[routes]
impl UserController {
    #[get("/")]
    async fn list(&self) {}
}
"#,
    )
    .unwrap();
    // mod.rs should be skipped
    fs::write("src/controllers/mod.rs", "pub mod user;\n").unwrap();

    assert!(routes::run().is_ok());
}

#[test]
fn parses_patch_method() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("test.rs");
    fs::write(
        &file_path,
        r#"
#[controller(path = "/items", state = AppState)]
pub struct ItemController;

#[routes]
impl ItemController {
    #[patch("/{id}")]
    async fn partial_update(&self) {}
}
"#,
    )
    .unwrap();

    let mut routes = Vec::new();
    parse_routes_from_file(&file_path, &mut routes).unwrap();

    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].method, "PATCH");
    assert_eq!(routes[0].handler, "partial_update");
}
