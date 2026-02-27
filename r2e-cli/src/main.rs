mod commands;

use clap::{Parser, Subcommand};
use commands::{add, dev, doctor, generate, new_project, routes};

#[derive(Parser)]
#[command(name = "r2e", version, about = "R2E CLI — scaffold and manage R2E projects")]
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
        /// Extension name (e.g. security, data, openapi, events, scheduler)
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
    /// Check project health
    Doctor,
    /// List all declared routes
    Routes,
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
        Commands::Doctor => doctor::run(),
        Commands::Routes => routes::run(),
    };

    if let Err(e) = result {
        eprintln!("{}", colored::Colorize::red(format!("Error: {e}").as_str()));
        std::process::exit(1);
    }
}
