use colored::Colorize;
use std::fs;
use std::path::Path;

use super::templates::{self, to_snake_case, pluralize};

pub fn controller(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let file_name = to_snake_case(name);
    let path = Path::new("src/controllers").join(format!("{file_name}.rs"));

    if path.exists() {
        return Err(format!("Controller file '{}' already exists", path.display()).into());
    }

    if !Path::new("src/controllers").exists() {
        fs::create_dir_all("src/controllers")?;
    }

    let content = format!(
        r#"use axum::Json;
use r2e_core::prelude::*;
use serde::{{Deserialize, Serialize}};

// TODO: import your state type
// use crate::state::AppState;

#[derive(Controller)]
#[controller(state = AppState)]
pub struct {name} {{
    // #[inject]
    // your_service: YourService,
}}

#[routes]
impl {name} {{
    #[get("/your-path")]
    async fn list(&self) -> Json<String> {{
        Json("Hello from {name}".into())
    }}
}}
"#
    );

    fs::write(&path, content)?;

    println!(
        "{} Generated controller: {}",
        "✓".green(),
        path.display().to_string().cyan()
    );

    // Update mod.rs
    let mod_path = Path::new("src/controllers/mod.rs");
    if mod_path.exists() {
        let existing = fs::read_to_string(mod_path)?;
        let mod_line = format!("pub mod {file_name};\n");
        if !existing.contains(&mod_line) {
            fs::write(mod_path, format!("{existing}{mod_line}"))?;
            println!("{} Updated src/controllers/mod.rs", "✓".green());
        }
    }

    Ok(())
}

pub fn service(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let file_name = to_snake_case(name);
    let path = Path::new("src").join(format!("{file_name}.rs"));

    if path.exists() {
        return Err(format!("Service file '{}' already exists", path.display()).into());
    }

    let content = format!(
        r#"use std::sync::Arc;

#[derive(Clone)]
pub struct {name} {{
    // Add your dependencies here
}}

impl {name} {{
    pub fn new() -> Self {{
        Self {{}}
    }}

    // Add your methods here
}}
"#
    );

    fs::write(&path, content)?;

    println!(
        "{} Generated service: {}",
        "✓".green(),
        path.display().to_string().cyan()
    );

    Ok(())
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub rust_type: String,
    pub is_optional: bool,
}

fn parse_fields(fields: &[String]) -> Result<Vec<Field>, Box<dyn std::error::Error>> {
    fields
        .iter()
        .map(|f| {
            let parts: Vec<&str> = f.split(':').collect();
            if parts.len() != 2 {
                return Err(
                    format!("Invalid field format '{}'. Expected 'name:Type'", f).into(),
                );
            }
            let name = parts[0].to_string();
            let rust_type = parts[1].to_string();
            let is_optional = rust_type.starts_with("Option<");
            Ok(Field {
                name,
                rust_type,
                is_optional,
            })
        })
        .collect()
}

pub fn crud(name: &str, raw_fields: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let fields = parse_fields(raw_fields)?;
    let snake = to_snake_case(name);
    let plural = pluralize(&snake);

    println!("{} Generating CRUD for '{}'", "->".blue(), name.green());

    // 1. Model
    let model_dir = Path::new("src/models");
    fs::create_dir_all(model_dir)?;
    let model_path = model_dir.join(format!("{snake}.rs"));
    fs::write(&model_path, crud_model(name, &fields))?;
    update_mod_rs(model_dir, &snake)?;
    println!("  {} {}", "✓".green(), model_path.display());

    // 2. Service
    let service_dir = Path::new("src/services");
    fs::create_dir_all(service_dir)?;
    let service_path = service_dir.join(format!("{snake}_service.rs"));
    fs::write(&service_path, crud_service(name, &fields))?;
    update_mod_rs(service_dir, &format!("{snake}_service"))?;
    println!("  {} {}", "✓".green(), service_path.display());

    // 3. Controller
    let controller_dir = Path::new("src/controllers");
    fs::create_dir_all(controller_dir)?;
    let controller_path = controller_dir.join(format!("{snake}_controller.rs"));
    fs::write(&controller_path, crud_controller(name, &fields))?;
    update_mod_rs(controller_dir, &format!("{snake}_controller"))?;
    println!("  {} {}", "✓".green(), controller_path.display());

    // 4. Migration (if migrations/ directory exists)
    if Path::new("migrations").exists() {
        let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
        let migration_path =
            Path::new("migrations").join(format!("{}_create_{plural}.sql", timestamp));
        fs::write(&migration_path, crud_migration(name, &fields))?;
        println!("  {} {}", "✓".green(), migration_path.display());
    }

    // 5. Test
    let test_dir = Path::new("tests");
    fs::create_dir_all(test_dir)?;
    let test_path = test_dir.join(format!("{snake}_test.rs"));
    fs::write(&test_path, crud_test(name))?;
    println!("  {} {}", "✓".green(), test_path.display());

    println!();
    println!(
        "{} CRUD generated for '{}'!",
        "✓".green(),
        name.green()
    );
    println!();
    println!("  Next steps:");
    println!("  1. Register the controller in main.rs:");
    println!(
        "     {}",
        format!(".register_controller::<{name}Controller>()").cyan()
    );
    println!("  2. Add {name}Service to your state struct");
    println!("  3. Run migrations if applicable");
    println!("  4. Run `cargo check` to verify");

    Ok(())
}

fn update_mod_rs(dir: &Path, module_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mod_path = dir.join("mod.rs");
    let mod_line = format!("pub mod {module_name};\n");

    if mod_path.exists() {
        let existing = fs::read_to_string(&mod_path)?;
        if !existing.contains(&mod_line) {
            fs::write(&mod_path, format!("{existing}{mod_line}"))?;
        }
    } else {
        fs::write(&mod_path, &mod_line)?;
    }
    Ok(())
}

fn crud_model(entity_name: &str, fields: &[Field]) -> String {
    let fields_str: String = fields
        .iter()
        .map(|f| format!("    pub {}: {},\n", f.name, f.rust_type))
        .collect();

    let create_fields: String = fields
        .iter()
        .filter(|f| f.name != "id")
        .map(|f| format!("    pub {}: {},\n", f.name, f.rust_type))
        .collect();

    format!(
        r#"use serde::{{Deserialize, Serialize}};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct {entity_name} {{
    pub id: i64,
{fields_str}}}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Create{entity_name}Request {{
{create_fields}}}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Update{entity_name}Request {{
{create_fields}}}
"#
    )
}

fn crud_service(entity_name: &str, _fields: &[Field]) -> String {
    let snake = to_snake_case(entity_name);
    let plural = pluralize(&snake);

    format!(
        r#"use crate::models::{snake}::{{{entity_name}, Create{entity_name}Request, Update{entity_name}Request}};
use r2e::prelude::*;
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct {entity_name}Service {{
    pool: SqlitePool,
}}

#[bean]
impl {entity_name}Service {{
    pub fn new(pool: SqlitePool) -> Self {{
        Self {{ pool }}
    }}

    pub async fn list(&self) -> Vec<{entity_name}> {{
        sqlx::query_as!(
            {entity_name},
            "SELECT * FROM {plural}"
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }}

    pub async fn get_by_id(&self, id: i64) -> Option<{entity_name}> {{
        sqlx::query_as!(
            {entity_name},
            "SELECT * FROM {plural} WHERE id = ?",
            id
        )
        .fetch_optional(&self.pool)
        .await
        .unwrap_or(None)
    }}

    pub async fn create(&self, req: Create{entity_name}Request) -> {entity_name} {{
        // TODO: implement insert
        todo!("Implement create")
    }}

    pub async fn update(&self, id: i64, req: Update{entity_name}Request) -> Option<{entity_name}> {{
        // TODO: implement update
        todo!("Implement update")
    }}

    pub async fn delete(&self, id: i64) -> bool {{
        let result = sqlx::query!("DELETE FROM {plural} WHERE id = ?", id)
            .execute(&self.pool)
            .await;
        matches!(result, Ok(r) if r.rows_affected() > 0)
    }}
}}
"#
    )
}

fn crud_controller(entity_name: &str, _fields: &[Field]) -> String {
    let snake = to_snake_case(entity_name);
    let plural = pluralize(&snake);

    format!(
        r#"use crate::models::{snake}::{{{entity_name}, Create{entity_name}Request, Update{entity_name}Request}};
use crate::services::{snake}_service::{entity_name}Service;
use r2e::prelude::*;

#[derive(Controller)]
#[controller(path = "/{plural}", state = AppState)]
pub struct {entity_name}Controller {{
    #[inject]
    service: {entity_name}Service,
}}

#[routes]
impl {entity_name}Controller {{
    #[get("/")]
    async fn list(&self) -> Json<Vec<{entity_name}>> {{
        Json(self.service.list().await)
    }}

    #[get("/{{id}}")]
    async fn get_by_id(&self, Path(id): Path<i64>) -> Result<Json<{entity_name}>, AppError> {{
        self.service
            .get_by_id(id)
            .await
            .map(Json)
            .ok_or_else(|| AppError::NotFound("{entity_name} not found".into()))
    }}

    #[post("/")]
    async fn create(&self, Json(body): Json<Create{entity_name}Request>) -> Json<{entity_name}> {{
        Json(self.service.create(body).await)
    }}

    #[put("/{{id}}")]
    async fn update(
        &self,
        Path(id): Path<i64>,
        Json(body): Json<Update{entity_name}Request>,
    ) -> Result<Json<{entity_name}>, AppError> {{
        self.service
            .update(id, body)
            .await
            .map(Json)
            .ok_or_else(|| AppError::NotFound("{entity_name} not found".into()))
    }}

    #[delete("/{{id}}")]
    async fn delete(&self, Path(id): Path<i64>) -> Result<Json<&'static str>, AppError> {{
        if self.service.delete(id).await {{
            Ok(Json("deleted"))
        }} else {{
            Err(AppError::NotFound("{entity_name} not found".into()))
        }}
    }}
}}
"#
    )
}

fn crud_migration(entity_name: &str, fields: &[Field]) -> String {
    let snake = to_snake_case(entity_name);
    let plural = pluralize(&snake);

    let columns: String = fields
        .iter()
        .map(|f| {
            let sql_type = rust_type_to_sql(&f.rust_type);
            let nullable = if f.is_optional { "" } else { " NOT NULL" };
            format!("    {} {}{}", f.name, sql_type, nullable)
        })
        .collect::<Vec<_>>()
        .join(",\n");

    format!(
        r#"-- Create {plural} table
CREATE TABLE IF NOT EXISTS {plural} (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
{columns}
);
"#
    )
}

fn rust_type_to_sql(rust_type: &str) -> &str {
    let inner = rust_type
        .strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(rust_type);

    match inner {
        "String" | "&str" => "TEXT",
        "i32" | "i64" | "u32" | "u64" | "usize" => "INTEGER",
        "f32" | "f64" => "REAL",
        "bool" => "BOOLEAN",
        _ => "TEXT",
    }
}

fn crud_test(entity_name: &str) -> String {
    let snake = to_snake_case(entity_name);
    let plural = pluralize(&snake);

    format!(
        r#"use r2e_test::{{TestApp, TestJwt}};
use serde_json::json;

// TODO: adapt imports to your project structure
// use crate::models::{snake}::{entity_name};

#[tokio::test]
async fn test_list_{plural}() {{
    // let app = TestApp::from_builder(/* your builder */);
    // let resp = app.get("/{plural}").send().await;
    // resp.assert_ok();
    todo!("Setup TestApp and test list endpoint");
}}

#[tokio::test]
async fn test_create_{snake}() {{
    // let app = TestApp::from_builder(/* your builder */);
    // let body = json!({{ /* fields */ }});
    // let resp = app.post("/{plural}").json(&body).send().await;
    // resp.assert_created();
    todo!("Setup TestApp and test create endpoint");
}}

#[tokio::test]
async fn test_get_{snake}_not_found() {{
    // let app = TestApp::from_builder(/* your builder */);
    // let resp = app.get("/{plural}/999").send().await;
    // resp.assert_not_found();
    todo!("Setup TestApp and test 404");
}}

#[tokio::test]
async fn test_delete_{snake}() {{
    // let app = TestApp::from_builder(/* your builder */);
    // let resp = app.delete("/{plural}/1").send().await;
    // resp.assert_ok();
    todo!("Setup TestApp and test delete");
}}
"#
    )
}

pub fn grpc_service(name: &str, package: &str) -> Result<(), Box<dyn std::error::Error>> {
    let snake = to_snake_case(name);

    println!(
        "{} Generating gRPC service '{}'",
        "->".blue(),
        name.green()
    );

    // 1. Proto file
    let proto_dir = Path::new("proto");
    fs::create_dir_all(proto_dir)?;
    let proto_path = proto_dir.join(format!("{snake}.proto"));
    if proto_path.exists() {
        return Err(format!("Proto file '{}' already exists", proto_path.display()).into());
    }
    fs::write(&proto_path, grpc_proto(name, package))?;
    println!("  {} {}", "✓".green(), proto_path.display());

    // 2. Rust service file
    let service_dir = Path::new("src/grpc");
    fs::create_dir_all(service_dir)?;
    let service_path = service_dir.join(format!("{snake}.rs"));
    if service_path.exists() {
        return Err(format!("Service file '{}' already exists", service_path.display()).into());
    }
    fs::write(&service_path, grpc_service_rs(name, package, &snake))?;
    update_mod_rs(service_dir, &snake)?;
    println!("  {} {}", "✓".green(), service_path.display());

    println!();
    println!(
        "{} gRPC service '{}' generated!",
        "✓".green(),
        name.green()
    );
    println!();
    println!("  Next steps:");
    println!(
        "  1. Add to build.rs: {}",
        format!("tonic_build::compile_protos(\"proto/{snake}.proto\")?;").cyan()
    );
    println!(
        "  2. Register in main.rs: {}",
        format!(".register_grpc_service::<{name}Service>()").cyan()
    );
    println!("  3. Run `cargo build` to generate proto code");

    Ok(())
}

fn grpc_proto(name: &str, package: &str) -> String {
    format!(
        r#"syntax = "proto3";

package {package};

service {name} {{
  rpc Get{name} (Get{name}Request) returns (Get{name}Response);
  rpc List{name} (List{name}Request) returns (List{name}Response);
}}

message Get{name}Request {{
  string id = 1;
}}

message Get{name}Response {{
  string id = 1;
  string name = 2;
}}

message List{name}Request {{
  int32 page_size = 1;
  string page_token = 2;
}}

message List{name}Response {{
  repeated Get{name}Response items = 1;
  string next_page_token = 2;
}}
"#
    )
}

fn grpc_service_rs(name: &str, package: &str, snake: &str) -> String {
    format!(
        r#"use r2e::prelude::*;
use r2e::r2e_grpc::AppBuilderGrpcExt;

pub mod proto {{
    tonic::include_proto!("{package}");
}}

use proto::{snake}_server::{name};
use proto::*;

// TODO: import your state type
// use crate::state::AppState;

#[derive(Controller)]
#[controller(state = AppState)]
pub struct {name}Service {{
    // #[inject]
    // your_dependency: YourDependency,
}}

#[grpc_routes(proto::{snake}_server::{name})]
impl {name}Service {{
    async fn get_{snake}(
        &self,
        request: tonic::Request<Get{name}Request>,
    ) -> Result<tonic::Response<Get{name}Response>, tonic::Status> {{
        let req = request.into_inner();
        let reply = Get{name}Response {{
            id: req.id,
            name: "TODO".to_string(),
        }};
        Ok(tonic::Response::new(reply))
    }}

    async fn list_{snake}(
        &self,
        _request: tonic::Request<List{name}Request>,
    ) -> Result<tonic::Response<List{name}Response>, tonic::Status> {{
        let reply = List{name}Response {{
            items: vec![],
            next_page_token: String::new(),
        }};
        Ok(tonic::Response::new(reply))
    }}
}}
"#
    )
}

pub fn middleware(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let file_name = to_snake_case(name);

    let middleware_dir = Path::new("src/middleware");
    fs::create_dir_all(middleware_dir)?;

    let path = middleware_dir.join(format!("{file_name}.rs"));
    if path.exists() {
        return Err(format!("Middleware file '{}' already exists", path.display()).into());
    }

    let content = templates::middleware::interceptor(name);
    fs::write(&path, content)?;

    println!(
        "{} Generated middleware: {}",
        "✓".green(),
        path.display().to_string().cyan()
    );

    // Update mod.rs
    update_mod_rs(middleware_dir, &file_name)?;
    println!("{} Updated src/middleware/mod.rs", "✓".green());

    Ok(())
}

