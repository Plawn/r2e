mod commands;

use clap::{Parser, Subcommand};
use commands::{add, dev, generate, new_project};

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
    },
    /// Generate a controller or service
    Generate {
        #[command(subcommand)]
        kind: GenerateKind,
    },
    /// Add an extension to the project
    Add {
        /// Extension name (e.g. security, data, openapi, events, scheduler)
        extension: String,
    },
    /// Start the dev server with hot-reload
    Dev,
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
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::New { name } => new_project::run(&name),
        Commands::Generate { kind } => match kind {
            GenerateKind::Controller { name } => generate::controller(&name),
            GenerateKind::Service { name } => generate::service(&name),
        },
        Commands::Add { extension } => add::run(&extension),
        Commands::Dev => dev::run(),
    };

    if let Err(e) = result {
        eprintln!("{}", colored::Colorize::red(format!("Error: {e}").as_str()));
        std::process::exit(1);
    }
}
