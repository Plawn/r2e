mod commands;

use clap::{Parser, Subcommand};
use commands::{add, dev, docs, doctor, generate, llm_docs, new_project, routes, test};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "r2e",
    version,
    about = "R2E CLI — scaffold and manage R2E projects"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new R2E project
    New {
        /// Project name
        name: String,
        /// Include database support (sqlite, postgres, mysql)
        #[arg(long)]
        db: Option<String>,
        /// Include JWT/OIDC security
        #[arg(long)]
        auth: bool,
        /// Include OpenAPI docs
        #[arg(long)]
        openapi: bool,
        /// Include Prometheus metrics
        #[arg(long)]
        metrics: bool,
        /// Include gRPC server support
        #[arg(long)]
        grpc: bool,
        /// Include all features
        #[arg(long)]
        full: bool,
        /// Skip interactive prompts (use defaults)
        #[arg(long)]
        no_interactive: bool,
    },
    /// Generate a controller, service, CRUD, or middleware
    Generate {
        #[command(subcommand)]
        kind: GenerateKind,
    },
    /// Add an extension to the project
    Add {
        /// Extension name (e.g. security, data-sqlx, openapi, events, scheduler)
        extension: String,
    },
    /// Start the dev server with Subsecond hot-reload
    Dev {
        /// Server port (forwarded as R2E_PORT env var)
        #[arg(long)]
        port: Option<u16>,
        /// Extra Cargo features to enable
        #[arg(long, num_args = 1..)]
        features: Vec<String>,
    },
    /// Run tests, optionally with coverage for SonarQube
    Test {
        /// Run tests with cargo-llvm-cov
        #[arg(long)]
        coverage: bool,
        /// Generate an LCOV report consumable by SonarQube
        #[arg(long)]
        sonarqube: bool,
        /// LCOV output path for --sonarqube
        #[arg(long)]
        output_path: Option<PathBuf>,
        /// Test all workspace packages
        #[arg(long)]
        workspace: bool,
        /// Package to test
        #[arg(long = "package", short = 'p')]
        packages: Vec<String>,
        /// Extra Cargo features to enable
        #[arg(long, num_args = 1..)]
        features: Vec<String>,
        /// Activate all available features
        #[arg(long)]
        all_features: bool,
        /// Do not activate default features
        #[arg(long)]
        no_default_features: bool,
        /// Arguments forwarded to the test binary after `--`
        #[arg(last = true, allow_hyphen_values = true, num_args = 0..)]
        test_args: Vec<String>,
    },
    /// Check project health
    Doctor,
    /// List all declared routes
    Routes,
    /// Print module documentation (bundled, version-matched)
    Docs {
        /// Module slug or crate name (omit to list all); with --llm, a topic slug
        module: Option<String>,
        /// Print the full doc instead of just the TL;DR (with --llm: the single-file reference)
        #[arg(long)]
        full: bool,
        /// Render markdown for a terminal instead of raw output
        #[arg(long, short)]
        pretty: bool,
        /// Use the AI/agent-facing reference (llm.txt + llm/<topic>.md) instead of module docs
        #[arg(long)]
        llm: bool,
        /// With --llm: write the whole reference into DIR (default docs/r2e) for agents to read locally
        #[arg(long, value_name = "DIR", num_args = 0..=1, default_missing_value = llm_docs::DEFAULT_EXPORT_DIR, requires = "llm")]
        export: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum GenerateKind {
    /// Generate a new controller
    Controller {
        /// Controller name (e.g. UserController)
        name: String,
    },
    /// Generate a new service
    Service {
        /// Service name (e.g. UserService)
        name: String,
    },
    /// Generate a complete CRUD (controller + service + model + tests)
    Crud {
        /// Entity name in PascalCase (e.g. User, BlogPost)
        name: String,
        /// Fields in format "name:type" (e.g. "name:String email:String age:i64")
        #[arg(long, num_args = 1..)]
        fields: Vec<String>,
    },
    /// Generate a middleware/interceptor
    Middleware {
        /// Middleware name (e.g. AuditLog)
        name: String,
    },
    /// Generate a gRPC service (.proto + Rust service)
    GrpcService {
        /// Service name in PascalCase (e.g. UserService)
        name: String,
        /// Proto package name (e.g. myapp)
        #[arg(long, default_value = "myapp")]
        package: String,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::New {
            name,
            db,
            auth,
            openapi,
            metrics,
            grpc,
            full,
            no_interactive,
        } => new_project::run(
            &name,
            new_project::CliNewOpts {
                db,
                auth,
                openapi,
                metrics,
                grpc,
                full,
                no_interactive,
            },
        ),
        Commands::Generate { kind } => match kind {
            GenerateKind::Controller { name } => generate::controller(&name),
            GenerateKind::Service { name } => generate::service(&name),
            GenerateKind::Crud { name, fields } => generate::crud(&name, &fields),
            GenerateKind::Middleware { name } => generate::middleware(&name),
            GenerateKind::GrpcService { name, package } => generate::grpc_service(&name, &package),
        },
        Commands::Add { extension } => add::run(&extension),
        Commands::Dev { port, features } => dev::run(port, features),
        Commands::Test {
            coverage,
            sonarqube,
            output_path,
            workspace,
            packages,
            features,
            all_features,
            no_default_features,
            test_args,
        } => test::run(test::TestOptions {
            coverage,
            sonarqube,
            output_path,
            workspace,
            packages,
            features,
            all_features,
            no_default_features,
            test_args,
        }),
        Commands::Doctor => doctor::run(),
        Commands::Routes => routes::run(),
        Commands::Docs {
            module,
            full,
            pretty,
            llm: true,
            export,
        } => llm_docs::run(module.as_deref(), full, export.as_deref(), pretty),
        Commands::Docs {
            module,
            full,
            pretty,
            llm: false,
            ..
        } => docs::run(module.as_deref(), full, pretty),
    };

    if let Err(e) = result {
        eprintln!("{}", colored::Colorize::red(format!("Error: {e}").as_str()));
        std::process::exit(1);
    }
}
