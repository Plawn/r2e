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

    let openapi_dep = if opts.openapi {
        "\nschemars = \"1\""
    } else {
        ""
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
tracing-subscriber = {{ version = "0.3", features = ["env-filter"] }}{db_dep}{openapi_dep}{grpc_deps}
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
        imports.push_str("use r2e::r2e_grpc::GrpcServer;\n");
    }
    if opts.auth {
        imports.push_str("use std::sync::Arc;\n");
        imports
            .push_str("use r2e::r2e_security::{JwksCache, JwtClaimsValidator, SecurityConfig};\n");
    }

    imports.push_str("\nmod controllers;\n\n");
    imports.push_str("use controllers::hello::HelloController;\n");

    // Producers — construct config-driven beans that feed the DI graph.
    // Each `#[producer] async fn foo(...)` generates a `Foo` bean type
    // registered below with `.register::<Foo>()`.
    let mut producers = String::new();
    if let Some(db) = &opts.db {
        let pool_ty = match db {
            DbKind::Sqlite => "sqlx::SqlitePool",
            DbKind::Postgres => "sqlx::PgPool",
            DbKind::Mysql => "sqlx::MySqlPool",
        };
        producers.push_str(&format!(
            r#"
#[producer]
async fn create_pool(#[config("database.url")] url: String) -> {pool_ty} {{
    {pool_ty}::connect(&url)
        .await
        .expect("Failed to connect to the database")
}}
"#
        ));
    }
    if opts.auth {
        producers.push_str(
            r#"
#[producer]
async fn jwt_validator(
    #[config("security.jwt.jwks-url")] jwks_url: String,
    #[config("security.jwt.issuer")] issuer: String,
    #[config("security.jwt.audience")] audience: String,
) -> Arc<JwtClaimsValidator> {
    let config = SecurityConfig::new(jwks_url, issuer, audience);
    let jwks = Arc::new(
        JwksCache::new(config.clone())
            .await
            .expect("Failed to initialize JWKS cache"),
    );
    Arc::new(JwtClaimsValidator::new(jwks, config))
}
"#,
        );
    }

    // Builder chain. Beans are `.provide()`-d or `.register()`-ed before
    // `.build_state().await`, which infers the application state from the
    // provision list. Plugins and controllers are wired afterward.
    let mut lines: Vec<String> = Vec::new();
    lines.push("    AppBuilder::new()".to_string());

    if opts.scheduler {
        lines.push("        .plugin(Scheduler)".to_string());
    }
    if opts.grpc {
        lines.push("        .plugin(GrpcServer::on_port(\"0.0.0.0:50051\"))".to_string());
    }
    if opts.db.is_some() || opts.auth {
        // Config-driven producers need the runtime config loaded first.
        lines.push("        .load_config::<()>()".to_string());
    }
    if opts.events {
        lines.push("        .provide(LocalEventBus::new())".to_string());
    }
    if opts.db.is_some() {
        lines.push("        .register::<CreatePool>()".to_string());
    }
    if opts.auth {
        lines.push("        .register::<JwtValidator>()".to_string());
    }

    lines.push("        .build_state()".to_string());
    lines.push("        .await".to_string());
    lines.push("        .with(Health)".to_string());
    lines.push("        .with(Tracing)".to_string());

    if opts.openapi {
        lines.push(
            "        .with(OpenApiPlugin::new(OpenApiConfig::new(\"API\", \"0.1.0\").with_docs_ui(true)))"
                .to_string(),
        );
    }

    lines.push("        .register_controller::<HelloController>()".to_string());
    lines.push("        .serve(\"0.0.0.0:3000\")".to_string());
    lines.push("        .await".to_string());
    lines.push("        .unwrap();".to_string());

    let builder = lines.join("\n");

    format!(
        r#"// #![recursion_limit = "512"]  // uncomment if you register more than ~127 beans
{imports}{producers}
#[r2e::main]
async fn main() {{
{builder}
}}
"#
    )
}

pub fn application_yaml(opts: &ProjectOptions) -> String {
    let name = &opts.name;
    let mut yaml = format!(
        r#"app:
  name: "{name}"
  port: 3000
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
    r#"use r2e::prelude::*;

#[controller(path = "/")]
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
