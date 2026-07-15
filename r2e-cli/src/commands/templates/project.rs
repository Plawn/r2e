use super::super::new_project::{DbKind, ProjectOptions};

/// Source for the `r2e` / `r2e-test` crates. R2E is not published to crates.io
/// yet, so scaffolded projects depend on the GitHub repository directly.
/// Switch these to `version = "x.y"` once the crates are published.
const R2E_GIT: &str = "https://github.com/Plawn/r2e";

/// Rust identifier for the crate (Cargo maps `-` to `_` in target names).
fn crate_ident(name: &str) -> String {
    name.replace('-', "_")
}

/// PascalCase name of the generated `App` struct, derived from the crate name.
/// Single source of truth so the generated lib.rs, main.rs, integration test,
/// and AGENTS.md can never disagree on what the app struct is called.
fn app_ident(name: &str) -> String {
    super::to_pascal_case(&crate_ident(name))
}

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
r2e = {{ git = "{R2E_GIT}"{features_str} }}
tokio = {{ version = "1", features = ["full"] }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
tracing = "0.1"
tracing-subscriber = {{ version = "0.3", features = ["env-filter"] }}{db_dep}{openapi_dep}{grpc_deps}

[dev-dependencies]
r2e-test = {{ git = "{R2E_GIT}" }}
{grpc_build_deps}"#
    )
}

/// The application declaration (`lib.rs`). The `App` impl lives here so
/// integration tests can boot the exact same app via
/// `#[r2e::test(app = <crate>::<App>)]`.
pub fn lib_rs(opts: &ProjectOptions) -> String {
    let mut imports = String::from("use r2e::prelude::*;\n");

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

    imports.push_str("\npub mod controllers;\n\n");
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
    lines.push("        b".to_string());

    if opts.scheduler {
        lines.push("            .plugin(Scheduler)".to_string());
    }
    if opts.grpc {
        lines.push("            .plugin(GrpcServer::on_port(\"0.0.0.0:50051\"))".to_string());
    }
    // serve_auto() reads server.host/server.port, so config is always loaded.
    lines.push("            .load_config::<()>()".to_string());
    if opts.events {
        lines.push("            .provide(LocalEventBus::new())".to_string());
    }
    if opts.db.is_some() {
        lines.push("            .register::<CreatePool>()".to_string());
    }
    if opts.auth {
        lines.push("            .register::<JwtValidator>()".to_string());
    }

    lines.push("            .build_state()".to_string());
    lines.push("            .await".to_string());
    lines.push("            .with(Health)".to_string());
    lines.push("            .with(Tracing)".to_string());

    if opts.openapi {
        lines.push(
            "            .with(OpenApiPlugin::new(OpenApiConfig::new(\"API\", \"0.1.0\").with_docs_ui(true)))"
                .to_string(),
        );
    }

    lines.push("            .register_controller::<HelloController>()".to_string());

    let builder = lines.join("\n");
    let app_name = app_ident(&opts.name);

    format!(
        r#"// #![recursion_limit = "512"]  // uncomment if you register more than ~127 beans
{imports}{producers}
/// The application. `main.rs` launches this in production (and, with the
/// `dev-reload` feature, hot-reloads it); integration tests boot the **same**
/// unit via `#[r2e::test(app = ...)]` — never re-declare controllers in tests.
pub struct {app_name};

impl App for {app_name} {{
    /// Long-lived resources built once. In dev mode they survive hot-patches.
    /// This app has nothing to persist, so `Env` is `()` and `setup` is empty.
    type Env = ();

    async fn setup() {{}}

    /// Re-run on every hot-patch: assemble the app on the given builder.
    async fn build(b: AppBuilder, _env: Self::Env) -> impl BootableApp {{
{builder}
    }}
}}
"#
    )
}

/// Thin binary entry point — launches the app (prod serve, or hot-reload
/// under the `dev-reload` feature). `launch` reads `server.host`/`server.port`
/// from config and calls `serve_auto()` internally.
pub fn main_rs(opts: &ProjectOptions) -> String {
    let ident = crate_ident(&opts.name);
    let app_name = app_ident(&opts.name);
    format!(
        r#"use r2e::prelude::*;

#[r2e::main]
async fn main() {{
    r2e::launch::<{ident}::{app_name}>().await.unwrap();
}}
"#
    )
}

/// Integration test booting the real application — the same unit `main()`
/// launches.
pub fn app_test(opts: &ProjectOptions) -> String {
    let ident = crate_ident(&opts.name);
    let app_name = app_ident(&opts.name);
    format!(
        r#"//! Integration tests boot the real application — the same unit
//! `main()` launches. See AGENTS.md for the testing rules.

use r2e_test::TestApp;

#[r2e::test(app = {ident}::{app_name})]
async fn hello_works(app: TestApp) {{
    let resp = app.get("/").send().await;
    resp.assert_ok();
    assert_eq!(resp.text(), "Hello, World!");
}}

#[r2e::test(app = {ident}::{app_name})]
async fn health_works(app: TestApp) {{
    app.get("/health").send().await.assert_ok();
}}
"#
    )
}

pub fn application_yaml(opts: &ProjectOptions) -> String {
    let name = &opts.name;
    let mut yaml = format!(
        r#"app:
  name: "{name}"

# Read by serve_auto()
server:
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

/// Agent-facing instructions dropped into every generated project.
/// Keeps AI coding assistants on the idiomatic R2E path instead of
/// falling back to raw axum patterns.
pub fn agents_md(opts: &ProjectOptions) -> String {
    let ident = crate_ident(&opts.name);
    let app_name = app_ident(&opts.name);
    format!(
        r#"# AGENTS.md — working on this R2E project

This project uses [R2E](https://github.com/Plawn/r2e), a Quarkus-like
ergonomic layer over axum: declarative controllers, compile-time DI, and
zero runtime reflection. R2E wraps axum — **always reach for the R2E
construct first**; dropping to raw axum forfeits DI, guards, interceptors,
OpenAPI, and TestApp integration. The full AI-facing API reference is
`llm.txt` at the root of the R2E repository.

## Architecture rules

- **App trait**: the whole app is assembled in `src/lib.rs` in
  `impl App for {app_name}` — `build(b, env)` returns `impl BootableApp`, and
  long-lived resources go in `setup()` (they survive hot-patches). `src/main.rs`
  only calls `r2e::launch::<{app_name}>()`. Add new controllers/beans/plugins
  inside `build` — never in `main.rs`, and never build a second `AppBuilder`.
- **State is inferred**: there is no state struct. `.provide(bean)` /
  `.register::<T>()` before `.build_state().await`; inject by type with
  `#[inject]` fields. A missing bean is a compile error at
  `register_controller()`.
- **Endpoints are controllers**: a `#[controller(path = "...")]` struct +
  `#[routes]` impl per resource. New endpoint → new method on a controller
  (or a new controller registered in `App::build`).

## Do X, not Y (axum habits to avoid)

| You want | Do NOT write | Write instead |
|---|---|---|
| Auth on a route | A custom `FromRequestParts` extractor | `#[inject(identity)] user: AuthenticatedUser` |
| Public routes on a protected controller | Optional extractors everywhere | `#[anonymous]` on the route (fail-closed) |
| Authorization | Middleware / in-handler `if` | A `Guard` (`#[guard(...)]`) or `#[roles("admin")]` |
| Catch-all / proxy | `Router::fallback(handler)` | `#[fallback]` or `#[any("/prefix/{{*path}}")]` route |
| Shared services | `State<Arc<...>>` | `.provide()` / `.register::<T>()` + `#[inject]` |
| Config values | `std::env` / lazy statics | `#[config("key")]` or typed `ConfigProperties` sections |
| Logging/timing/caching | Tower middleware per concern | `#[intercept(Logged::info())]` / `Timed` / `Cache` |
| Background jobs | `tokio::spawn` in main | `#[scheduled(every = "5m")]` methods |
| Errors | Hand-rolled `IntoResponse` | `HttpError` or `#[derive(ApiError)]` |

## Testing rules

- Integration tests boot the **real app**:
  `#[r2e::test(app = {ident}::{app_name})] async fn t(app: TestApp)`.
  Never re-declare controllers or routers in tests.
- `.as_user("alice", &["admin"])` mints a valid JWT (no IdP needed).
- Pin mocks / patch config in the boot hook:
  `#[r2e::test(app = {ident}::{app_name}, with = |b| b.override_bean(MockMailer::new())
  .override_config_value("database.url", url))]`.
- Test-profile config goes in `application-test.yaml` (auto-overlaid).
- Tests live in `tests/`, one file per feature area.

## Commands

- `cargo check` / `cargo test` — verify.
- `r2e generate controller <Name>` — scaffold a controller.
- `r2e routes` — list registered routes. `r2e doctor` — diagnose setup.
- `r2e dev` — run with hot reload.
"#
    )
}

/// Claude Code reads CLAUDE.md; keep a single source of truth by importing
/// the tool-agnostic AGENTS.md.
pub fn claude_md() -> &'static str {
    "@AGENTS.md\n"
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
