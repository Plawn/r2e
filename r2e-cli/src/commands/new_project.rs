use colored::Colorize;
use dialoguer::{MultiSelect, Select};
use std::fs;
use std::path::Path;

use super::templates;

/// Supported database backends for project scaffolding.
#[derive(Debug, Clone)]
pub enum DbKind {
    Sqlite,
    Postgres,
    Mysql,
}

/// Resolved project options after CLI flag parsing or interactive prompts.
pub struct ProjectOptions {
    pub name: String,
    pub db: Option<DbKind>,
    pub auth: bool,
    pub openapi: bool,
    #[allow(dead_code)]
    pub metrics: bool,
    pub scheduler: bool,
    pub events: bool,
    pub grpc: bool,
}

/// Raw CLI flags for `r2e new`, before resolution into [`ProjectOptions`].
pub struct CliNewOpts {
    pub db: Option<String>,
    pub auth: bool,
    pub openapi: bool,
    pub metrics: bool,
    pub grpc: bool,
    pub full: bool,
    pub no_interactive: bool,
}

impl CliNewOpts {
    fn has_any_flag(&self) -> bool {
        self.db.is_some() || self.auth || self.openapi || self.metrics || self.grpc
    }
}

/// Create a new R2E project.
///
/// Resolves feature flags from `cli_opts`:
/// - `--full` enables all features (SQLite, auth, openapi, scheduler, events, gRPC).
/// - `--no-interactive` or any explicit flag uses provided values.
/// - Otherwise, prompts interactively with `dialoguer`.
///
/// Creates the project directory and all scaffold files (Cargo.toml, main.rs,
/// state.rs, hello controller, application.yaml, etc.).
pub fn run(name: &str, cli_opts: CliNewOpts) -> Result<(), Box<dyn std::error::Error>> {
    let opts = if cli_opts.full {
        ProjectOptions {
            name: name.to_string(),
            db: Some(DbKind::Sqlite),
            auth: true,
            openapi: true,
            metrics: false,
            scheduler: true,
            events: true,
            grpc: true,
        }
    } else if cli_opts.no_interactive || cli_opts.has_any_flag() {
        let db = cli_opts.db.as_deref().map(|d| match d {
            "sqlite" => DbKind::Sqlite,
            "postgres" | "pg" => DbKind::Postgres,
            "mysql" => DbKind::Mysql,
            _ => DbKind::Sqlite,
        });
        ProjectOptions {
            name: name.to_string(),
            db,
            auth: cli_opts.auth,
            openapi: cli_opts.openapi,
            metrics: cli_opts.metrics,
            scheduler: false,
            events: false,
            grpc: cli_opts.grpc,
        }
    } else {
        prompt_options(name)?
    };

    generate_project(&opts)
}

fn prompt_options(name: &str) -> Result<ProjectOptions, Box<dyn std::error::Error>> {
    println!(
        "{} Creating a new R2E project: {}",
        "->".blue(),
        name.green()
    );
    println!();

    // Database selection
    let db_choices = &["None", "SQLite", "PostgreSQL", "MySQL"];
    let db_idx = Select::new()
        .with_prompt("Database")
        .items(db_choices)
        .default(0)
        .interact()?;
    let db = match db_idx {
        1 => Some(DbKind::Sqlite),
        2 => Some(DbKind::Postgres),
        3 => Some(DbKind::Mysql),
        _ => None,
    };

    // Features selection
    let feature_choices = &[
        "JWT/OIDC Authentication",
        "OpenAPI Documentation",
        "Task Scheduling",
        "Event Bus",
        "gRPC Server",
    ];
    let selected = MultiSelect::new()
        .with_prompt("Select features (space to toggle, enter to confirm)")
        .items(feature_choices)
        .interact()?;

    Ok(ProjectOptions {
        name: name.to_string(),
        db,
        auth: selected.contains(&0),
        openapi: selected.contains(&1),
        metrics: false,
        scheduler: selected.contains(&2),
        events: selected.contains(&3),
        grpc: selected.contains(&4),
    })
}

fn generate_project(opts: &ProjectOptions) -> Result<(), Box<dyn std::error::Error>> {
    let project_dir = Path::new(&opts.name);
    if project_dir.exists() {
        return Err(format!("Directory '{}' already exists", opts.name).into());
    }

    println!(
        "{} Creating new R2E project: {}",
        "->".blue(),
        opts.name.green()
    );

    fs::create_dir_all(project_dir.join("src/controllers"))?;

    // 1. Cargo.toml
    fs::write(
        project_dir.join("Cargo.toml"),
        templates::project::cargo_toml(opts),
    )?;

    // 2. state.rs
    fs::write(
        project_dir.join("src/state.rs"),
        templates::project::state_rs(opts),
    )?;

    // 3. main.rs
    fs::write(
        project_dir.join("src/main.rs"),
        templates::project::main_rs(opts),
    )?;

    // 4. Hello controller
    fs::write(
        project_dir.join("src/controllers/hello.rs"),
        templates::project::hello_controller(),
    )?;
    fs::write(
        project_dir.join("src/controllers/mod.rs"),
        "pub mod hello;\n",
    )?;

    // 5. application.yaml
    fs::write(
        project_dir.join("application.yaml"),
        templates::project::application_yaml(opts),
    )?;

    // 6. Migrations directory if DB selected
    if opts.db.is_some() {
        fs::create_dir_all(project_dir.join("migrations"))?;
    }

    // 7. gRPC scaffolding
    if opts.grpc {
        fs::create_dir_all(project_dir.join("proto"))?;
        fs::write(
            project_dir.join("proto/greeter.proto"),
            templates::project::greeter_proto(&opts.name),
        )?;
        fs::write(
            project_dir.join("build.rs"),
            templates::project::build_rs(),
        )?;
    }

    // 8. .gitignore
    fs::write(project_dir.join(".gitignore"), "/target\n")?;

    println!(
        "{} Project '{}' created successfully!",
        "✓".green(),
        opts.name.green()
    );
    println!();
    println!("  cd {}", opts.name);
    println!("  cargo run");
    println!();

    if opts.openapi {
        println!(
            "  API docs: {}",
            "http://localhost:3000/docs".cyan()
        );
    }
    println!(
        "  Health:   {}",
        "http://localhost:3000/health".cyan()
    );

    if opts.grpc {
        println!(
            "  gRPC:     {}",
            "localhost:50051".cyan()
        );
    }

    Ok(())
}
