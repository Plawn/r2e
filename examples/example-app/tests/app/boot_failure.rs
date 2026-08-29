//! The entry-point contract for a boot that fails: a non-zero exit status and
//! ONE error message.
//!
//! The subject is `src/bin/boot_failure.rs`, assembled like any R2E binary
//! (`launch!` + `exit_on_boot_error`, i.e. what `app_main!` expands to). Run as
//! a real process, so the assertions are about what an operator or a supervisor
//! actually observes — not about the internals of the boot path.

use std::process::Command;

fn run_failing_boot() -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_boot_failure"))
        .env("RUST_BACKTRACE", "0")
        .output()
        .expect("the failing-boot binary runs")
}

#[test]
fn a_failing_boot_exits_non_zero_with_a_single_error_message() {
    let output = run_failing_boot();

    assert_eq!(
        output.status.code(),
        Some(1),
        "a boot failure must be a non-zero exit status, not 0 and not the 101 of a panic"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Exactly one `error:` line — the failure is reported once, not by every
    // layer it passes through.
    let errors: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("error: "))
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one error line, got: {stderr}"
    );
    assert_eq!(errors[0], "error: cannot read the database secret");

    // The cause chain is printed under it rather than flattened into the
    // message or dropped.
    assert!(
        stderr.contains("  caused by: /run/secrets/db-url: no such file"),
        "the source chain must be reported: {stderr}"
    );

    // An operational failure, not a bug: no panic, no backtrace advice.
    assert!(
        !stderr.contains("panicked"),
        "a boot failure must not panic: {stderr}"
    );
    assert!(
        !stderr.contains("RUST_BACKTRACE"),
        "a boot failure must not ask for a backtrace: {stderr}"
    );
}

// ── The same contract for the failures the framework raises ───────────────
//
// `setup` failing is the app's own error. These two are the framework's, and
// they reach the entry point through `?` on `try_build_state()` inside
// `App::build`: a bean constructor that fails, and a config that will not
// load. The subject is `src/bin/boot_failure_graph.rs`.

fn run_graph_boot(kind: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_boot_failure_graph"))
        .env("RUST_BACKTRACE", "0")
        .env("R2E_BOOT_FAILURE_KIND", kind)
        .output()
        .expect("the failing-boot binary runs")
}

/// Asserts the parts of the contract that do not depend on which step failed.
fn assert_operational_failure(output: &std::process::Output) -> String {
    assert_eq!(
        output.status.code(),
        Some(1),
        "a boot failure must be a non-zero exit status, not 0 and not the 101 of a panic"
    );

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let errors: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("error: "))
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one error line, got: {stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "a boot failure must not panic: {stderr}"
    );
    assert!(
        !stderr.contains("RUST_BACKTRACE"),
        "a boot failure must not ask for a backtrace: {stderr}"
    );
    stderr
}

#[test]
fn a_failing_bean_constructor_exits_like_any_other_boot_failure() {
    let output = run_graph_boot("producer");
    let stderr = assert_operational_failure(&output);

    // The bean is named (that is the point of wrapping in `BeanError`), and
    // the driver's own message is kept verbatim — once.
    assert!(
        stderr.contains("Bean 'boot_failure_graph::DbPool' failed to build"),
        "the failing bean must be named: {stderr}"
    );
    assert!(
        stderr.contains("could not connect to postgres://db:5432/app"),
        "the constructor's own error must survive: {stderr}"
    );
    assert_eq!(
        stderr.matches("could not connect to").count(),
        1,
        "the cause must be reported once, not echoed by every layer: {stderr}"
    );
}

#[test]
fn a_config_that_does_not_load_exits_like_any_other_boot_failure() {
    let output = run_graph_boot("config");
    let stderr = assert_operational_failure(&output);

    // `load_config()` cannot return a `Result` (type-state transition), so the
    // failure is recorded and surfaced by `try_build_state()` — before any
    // bean is built, which is why the deliberately-failing producer in that
    // binary never runs.
    assert!(
        stderr.contains("no-such-application-984.yaml"),
        "the offending config file must be named: {stderr}"
    );
    assert!(
        !stderr.contains("could not connect"),
        "config is validated before any bean is built: {stderr}"
    );
}
