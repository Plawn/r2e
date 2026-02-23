use super::super::new_project::{DbKind, ProjectOptions};

pub fn cargo_toml(opts: &ProjectOptions) -> String {
    let mut r2e_features = Vec::new();
    if opts.auth {
        r2e_features.push("security");
    }
    if opts.openapi {
        r2e_features.push("openapi");
    }
    if opts.events {
        r2e_features.push("events");
    }
    if opts.scheduler {
        r2e_features.push("scheduler");
    }
    if opts.db.is_some() {
        r2e_features.push("data");
    }
    if opts.grpc {
        r2e_features.push("grpc");
    }

    let features_str = if r2e_features.is_empty() {
        String::new()
    } else {
        format!(
            ", features = [{}]",
            r2e_features
                .iter()
                .map(|f| format!("\"{}\"", f))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    let db_dep = match &opts.db {
        Some(DbKind::Sqlite) => {
            "\nsqlx = { version = \"0.8\", features = [\"runtime-tokio\", \"sqlite\"] }"
        }
        Some(DbKind::Postgres) => {
            "\nsqlx = { version = \"0.8\", features = [\"runtime-tokio\", \"postgres\"] }"
        }
        Some(DbKind::Mysql) => {
            "\nsqlx = { version = \"0.8\", features = [\"runtime-tokio\", \"mysql\"] }"
        }
        None => "",
    };

    let grpc_deps = if opts.grpc {
        r#"
tonic = "~0.12"
prost = "~0.13""#
    } else {
        ""
    };

    let grpc_build_deps = if opts.grpc {
        r#"
[build-dependencies]
tonic-build = "~0.12"
"#
    } else {
        ""
    };

    let name = &opts.name;

    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[dependencies]
r2e = {{ version = "0.1"{features_str} }}
tokio = {{ version = "1", features = ["full"] }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
tracing = "0.1"
tracing-subscriber = {{ version = "0.3", features = ["env-filter"] }}{db_dep}{grpc_deps}
{grpc_build_deps}"#
    )
}

pub fn main_rs(opts: &ProjectOptions) -> String {
    let mut imports = String::from("use r2e::prelude::*;\n");
    imports.push_str("use r2e::plugins::{Health, Tracing};\n");

    if opts.openapi {
        imports.push_str("use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};\n");
    }
    if opts.scheduler {
        imports.push_str("use r2e::r2e_scheduler::Scheduler;\n");
    }
    if opts.grpc {
        imports.push_str("use r2e::r2e_grpc::{GrpcServer, AppBuilderGrpcExt};\n");
    }

    imports.push_str("\nmod controllers;\nmod state;\n\n");
    imports.push_str("use controllers::hello::HelloController;\n");
    imports.push_str("use state::AppState;\n");

    let mut builder_lines = Vec::new();
    builder_lines.push("    AppBuilder::new()".to_string());

    if opts.scheduler {
        builder_lines.push("        .plugin(Scheduler)".to_string());
    }
    if opts.grpc {
        builder_lines.push("        .plugin(GrpcServer::on_port(\"0.0.0.0:50051\"))".to_string());
    }

    builder_lines.push("        .build_state::<AppState, _, _>()".to_string());
    builder_lines.push("        .await".to_string());
    builder_lines.push("        .with(Health)".to_string());
    builder_lines.push("        .with(Tracing)".to_string());

    if opts.openapi {
        builder_lines.push(
            "        .with(OpenApiPlugin::new(OpenApiConfig::new(\"API\", \"0.1.0\").with_docs_ui(true)))"
                .to_string(),
        );
    }

    builder_lines.push("        .register_controller::<HelloController>()".to_string());
    builder_lines.push("        .serve(\"0.0.0.0:8080\")".to_string());
    builder_lines.push("        .await".to_string());
    builder_lines.push("        .unwrap();".to_string());

    let builder = builder_lines.join("\n");

    format!(
        r#"{imports}
#[r2e::main]
async fn main() {{
{builder}
}}
"#
    )
}

pub fn state_rs(opts: &ProjectOptions) -> String {
    let mut fields = String::new();
    let mut extra_imports = String::new();

    if opts.db.is_some() {
        match &opts.db {
            Some(DbKind::Sqlite) => {
                extra_imports.push_str("use sqlx::SqlitePool;\n");
                fields.push_str("    pub pool: SqlitePool,\n");
            }
            Some(DbKind::Postgres) => {
                extra_imports.push_str("use sqlx::PgPool;\n");
                fields.push_str("    pub pool: PgPool,\n");
            }
            Some(DbKind::Mysql) => {
                extra_imports.push_str("use sqlx::MySqlPool;\n");
                fields.push_str("    pub pool: MySqlPool,\n");
            }
            None => {}
        }
    }

    if opts.events {
        extra_imports.push_str("use r2e::r2e_events::EventBus;\n");
        fields.push_str("    pub event_bus: EventBus,\n");
    }

    if opts.auth {
        extra_imports.push_str("use r2e::r2e_security::JwtClaimsValidator;\n");
        extra_imports.push_str("use std::sync::Arc;\n");
        fields.push_str("    pub jwt_validator: Arc<JwtClaimsValidator>,\n");
    }

    let fields_block = if fields.is_empty() {
        String::new()
    } else {
        format!("\n{}", fields)
    };

    format!(
        r#"use r2e::prelude::*;
{extra_imports}
#[derive(Clone, BeanState)]
pub struct AppState {{{fields_block}}}
"#
    )
}

pub fn application_yaml(opts: &ProjectOptions) -> String {
    let name = &opts.name;
    let mut yaml = format!(
        r#"app:
  name: "{name}"
  port: 8080
"#
    );

    if let Some(db) = &opts.db {
        yaml.push('\n');
        match db {
            DbKind::Sqlite => {
                yaml.push_str("database:\n  url: \"sqlite:data.db?mode=rwc\"\n");
            }
            DbKind::Postgres => {
                yaml.push_str("database:\n  url: \"${DATABASE_URL}\"\n  pool-size: 10\n");
            }
            DbKind::Mysql => {
                yaml.push_str("database:\n  url: \"${DATABASE_URL}\"\n  pool-size: 10\n");
            }
        }
    }

    if opts.auth {
        yaml.push_str(
            "\nsecurity:\n  jwt:\n    issuer: \"my-app\"\n    audience: \"my-app\"\n    jwks-url: \"${JWKS_URL}\"\n",
        );
    }

    if opts.grpc {
        yaml.push_str("\ngrpc:\n  port: 50051\n");
    }

    yaml
}

pub fn hello_controller() -> &'static str {
    r#"use crate::state::AppState;
use r2e::prelude::*;

#[derive(Controller)]
#[controller(path = "/", state = AppState)]
pub struct HelloController;

#[routes]
impl HelloController {
    #[get("/")]
    async fn hello(&self) -> &'static str {
        "Hello, World!"
    }
}
"#
}

pub fn greeter_proto(project_name: &str) -> String {
    format!(
        r#"syntax = "proto3";

package {project_name};

service Greeter {{
  rpc SayHello (HelloRequest) returns (HelloReply);
}}

message HelloRequest {{
  string name = 1;
}}

message HelloReply {{
  string message = 1;
}}
"#
    )
}

pub fn build_rs() -> &'static str {
    r#"fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("proto/greeter.proto")?;
    Ok(())
}
"#
}
