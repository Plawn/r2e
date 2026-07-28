use colored::Colorize;
use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_SONAR_LCOV_PATH: &str = "coverage/lcov.info";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestOptions {
    pub coverage: bool,
    pub sonarqube: bool,
    pub output_path: Option<PathBuf>,
    pub workspace: bool,
    pub packages: Vec<String>,
    pub features: Vec<String>,
    pub all_features: bool,
    pub no_default_features: bool,
    pub test_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoInvocation {
    pub program: String,
    pub args: Vec<String>,
    pub sonarqube_report_path: Option<PathBuf>,
}

impl CargoInvocation {
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        command
    }
}

pub fn run(options: TestOptions) -> Result<(), Box<dyn std::error::Error>> {
    let invocation = build_invocation(&options);

    if let Some(path) = &invocation.sonarqube_report_path {
        ensure_parent_dir(path)?;
        println!(
            "{} generating SonarQube-compatible LCOV coverage at {}",
            "->".blue(),
            path.display()
        );
        println!(
            "{} configure SonarQube with: sonar.rust.lcov.reportPaths={}",
            "->".blue(),
            path.display()
        );
    } else if options.coverage {
        println!("{}", "-> running tests with cargo llvm-cov".blue());
    }

    let status = invocation.command().status()?;
    if status.success() {
        return Ok(());
    }

    if options.coverage || options.sonarqube {
        Err(format!(
            "cargo llvm-cov failed with status {status}. \
             Install it with `cargo install cargo-llvm-cov` and ensure \
             `rustup component add llvm-tools-preview` is available."
        )
        .into())
    } else {
        Err(format!("cargo test failed with status {status}").into())
    }
}

pub fn build_invocation(options: &TestOptions) -> CargoInvocation {
    let coverage = options.coverage || options.sonarqube;
    let sonarqube_report_path = options.sonarqube.then(|| {
        options
            .output_path
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_SONAR_LCOV_PATH))
    });

    let mut args = if coverage {
        vec!["llvm-cov".to_string()]
    } else {
        vec!["test".to_string()]
    };

    if options.workspace {
        args.push("--workspace".to_string());
    }

    for package in &options.packages {
        args.push("--package".to_string());
        args.push(package.clone());
    }

    if !options.features.is_empty() {
        args.push("--features".to_string());
        args.push(options.features.join(","));
    }

    if options.all_features {
        args.push("--all-features".to_string());
    }

    if options.no_default_features {
        args.push("--no-default-features".to_string());
    }

    if let Some(path) = &sonarqube_report_path {
        args.push("--lcov".to_string());
        args.push("--output-path".to_string());
        args.push(path.display().to_string());
    } else if coverage {
        // Plain `cargo llvm-cov` prints a summary to stdout; this mirrors
        // `cargo test` while keeping report generation opt-in.
    }

    if !options.test_args.is_empty() {
        args.push("--".to_string());
        args.extend(options.test_args.clone());
    }

    CargoInvocation {
        program: "cargo".to_string(),
        args,
        sonarqube_report_path,
    }
}

fn ensure_parent_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}
