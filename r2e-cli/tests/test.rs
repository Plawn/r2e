use r2e_cli::commands::test::{build_invocation, TestOptions};
use std::path::PathBuf;

fn opts() -> TestOptions {
    TestOptions {
        coverage: false,
        sonarqube: false,
        output_path: None,
        workspace: false,
        packages: Vec::new(),
        features: Vec::new(),
        all_features: false,
        no_default_features: false,
        test_args: Vec::new(),
    }
}

#[test]
fn default_runs_cargo_test() {
    let invocation = build_invocation(&opts());

    assert_eq!(invocation.program, "cargo");
    assert_eq!(invocation.args, vec!["test"]);
    assert_eq!(invocation.sonarqube_report_path, None);
}

#[test]
fn forwards_workspace_features_and_test_args() {
    let mut options = opts();
    options.workspace = true;
    options.features = vec!["sqlite".into(), "openapi".into()];
    options.test_args = vec!["--nocapture".into(), "filters::health".into()];

    let invocation = build_invocation(&options);

    assert_eq!(
        invocation.args,
        vec![
            "test",
            "--workspace",
            "--features",
            "sqlite,openapi",
            "--",
            "--nocapture",
            "filters::health",
        ]
    );
}

#[test]
fn coverage_runs_cargo_llvm_cov() {
    let mut options = opts();
    options.coverage = true;

    let invocation = build_invocation(&options);

    assert_eq!(invocation.args, vec!["llvm-cov"]);
    assert_eq!(invocation.sonarqube_report_path, None);
}

#[test]
fn sonarqube_generates_default_lcov_report() {
    let mut options = opts();
    options.sonarqube = true;

    let invocation = build_invocation(&options);

    assert_eq!(
        invocation.args,
        vec!["llvm-cov", "--lcov", "--output-path", "coverage/lcov.info",]
    );
    assert_eq!(
        invocation.sonarqube_report_path,
        Some(PathBuf::from("coverage/lcov.info"))
    );
}

#[test]
fn sonarqube_uses_custom_output_path() {
    let mut options = opts();
    options.sonarqube = true;
    options.output_path = Some(PathBuf::from("custom/lcov.info"));

    let invocation = build_invocation(&options);

    assert_eq!(
        invocation.args,
        vec!["llvm-cov", "--lcov", "--output-path", "custom/lcov.info"]
    );
    assert_eq!(
        invocation.sonarqube_report_path,
        Some(PathBuf::from("custom/lcov.info"))
    );
}

#[test]
fn forwards_package_and_feature_mode_flags() {
    let mut options = opts();
    options.packages = vec!["r2e-core".into(), "r2e-test".into()];
    options.all_features = true;
    options.no_default_features = true;

    let invocation = build_invocation(&options);

    assert_eq!(
        invocation.args,
        vec![
            "test",
            "--package",
            "r2e-core",
            "--package",
            "r2e-test",
            "--all-features",
            "--no-default-features",
        ]
    );
}
