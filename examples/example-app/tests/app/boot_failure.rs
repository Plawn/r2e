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
