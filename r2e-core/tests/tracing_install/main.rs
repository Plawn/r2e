//! Installing the process-global `tracing` subscriber: what R2E's entry points
//! install, and what a losing init reports.
//!
//! Its **own target** for the same reason as `tests/dev_reload/`: the global
//! subscriber is a one-shot process global, so who wins the install is a
//! property of the whole binary. Sharing a binary with any other test that
//! initialises tracing (`tests/runtime/tracing_config.rs` does) would make the
//! outcome depend on test scheduling. One test drives the sequence here, in
//! order.

use r2e_core::runtime::tracing_config::{LogFormat, SpanEvents, TracingConfig};
use r2e_core::{init_tracing_from_config, try_init_tracing_with_config, SubscriberAlreadyInstalled};

/// The whole point of #1010: the entry point installs the **application's**
/// `tracing:` section, not the built-in defaults, and a later init that would
/// have logged differently says so instead of vanishing.
#[test]
fn entry_point_installs_the_apps_section_and_later_inits_learn_they_lost() {
    let dir = std::env::temp_dir().join(format!("r2e-tracing-install-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("application.yaml"),
        "tracing:\n  format: json\n  filter: \"warn,r2e=debug\"\n  span-events: full\n",
    )
    .unwrap();
    std::env::set_current_dir(&dir).unwrap();

    let from_yaml = TracingConfig::default()
        .with_format(LogFormat::Json)
        .with_filter("warn,r2e=debug")
        .with_span_events(SpanEvents::Full);

    // What the entry point resolves before installing: the app's own section.
    let (resolved, problem) = r2e_core::runtime::layers::resolve_tracing_config();
    assert_eq!(resolved, from_yaml);
    assert_eq!(problem, None);

    // What `launch_with` / `#[r2e::main]` call.
    init_tracing_from_config();

    // The subscriber that won carries the app's section — `format: json`, not
    // the `Pretty` default.
    let lost = try_init_tracing_with_config(&from_yaml)
        .expect_err("a subscriber is already installed at this point");
    assert_eq!(lost.installed.as_ref(), Some(&from_yaml));

    // Re-installing the very same knobs is redundant but harmless: a
    // `ConfiguredTracing` plugin reading the same section must stay quiet.
    assert!(!lost.changes_output(&from_yaml));

    // Asking for something else is not harmless — that init produces none of
    // the output it asked for.
    let other = from_yaml.clone().with_format(LogFormat::Pretty);
    let lost = try_init_tracing_with_config(&other).expect_err("still installed");
    assert!(lost.changes_output(&other));

    // A `tracing:` section R2E cannot read must never cost the app its logs:
    // the defaults stand in, and the reason is carried back so the entry point
    // can say it out loud.
    std::fs::write(dir.join("application.yaml"), "tracing:\n  format: neon\n").unwrap();
    let (resolved, problem) = r2e_core::runtime::layers::resolve_tracing_config();
    assert_eq!(resolved, TracingConfig::default());
    assert!(
        problem.is_some_and(|p| p.contains("tracing")),
        "the fallback must name the section it could not read"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An unknown winner (a subscriber installed by the app or another library)
/// counts as a difference: nothing says it honours what was requested.
#[test]
fn an_unknown_winner_always_counts_as_a_difference() {
    let lost = SubscriberAlreadyInstalled { installed: None };
    assert!(lost.changes_output(&TracingConfig::default()));
    assert!(lost.to_string().contains("already"));
}
