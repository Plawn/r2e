use r2e_cli::commands::docs::{self, resolve_body, slugs, tldr};

// ── tldr() extraction ───────────────────────────────────────────────

#[test]
fn tldr_extracts_section_between_headings() {
    let body = "# Title\n\n## TL;DR\n\nShort summary line.\n\n## Goal\n\nThe rest.\n";
    let section = tldr(body).expect("TL;DR present");
    assert!(section.starts_with("## TL;DR"));
    assert!(section.contains("Short summary line."));
    // Must stop before the next heading.
    assert!(!section.contains("## Goal"));
    assert!(!section.contains("The rest."));
}

#[test]
fn tldr_runs_to_end_when_no_following_heading() {
    let body = "# Title\n\n## TL;DR\n\nOnly section, no more headings.\n";
    let section = tldr(body).expect("TL;DR present");
    assert!(section.contains("Only section, no more headings."));
}

#[test]
fn tldr_none_when_absent() {
    let body = "# Title\n\n## Goal\n\nNo TL;DR here.\n";
    assert!(tldr(body).is_none());
}

#[test]
fn tldr_ignores_mid_line_hash() {
    // A line mentioning "## TL;DR" inside prose must not be treated as the heading.
    let body = "# Title\n\nsee ## TL;DR below\n\n## TL;DR\n\nReal section.\n";
    let section = tldr(body).expect("TL;DR present");
    assert!(section.contains("Real section."));
}

// ── embedded corpus invariants ──────────────────────────────────────

#[test]
fn every_module_has_a_tldr() {
    for slug in slugs() {
        let body = resolve_body(slug).expect("slug resolves");
        assert!(
            tldr(body).is_some(),
            "module `{slug}` is missing a `## TL;DR` section",
        );
    }
}

#[test]
fn corpus_covers_all_features() {
    // One entry per docs/features/NN-*.md file.
    assert_eq!(slugs().len(), 23);
}

// ── slug lookup + known content ─────────────────────────────────────

#[test]
fn events_tldr_has_expected_content() {
    let body = resolve_body("events").expect("events slug");
    let section = tldr(body).expect("events TL;DR");
    assert!(section.contains("pub/sub"));
    assert!(section.contains("LocalEventBus"));
}

// ── crate-name aliases ──────────────────────────────────────────────

#[test]
fn crate_alias_resolves_one_to_one() {
    // `r2e-events` owns exactly one module → same body as the `events` slug.
    let by_alias = resolve_body("r2e-events").expect("alias resolves");
    let by_slug = resolve_body("events").expect("slug resolves");
    assert_eq!(by_alias, by_slug);
}

#[test]
fn crate_alias_with_many_modules_does_not_resolve_to_one() {
    // `r2e-core` owns several modules → ambiguous, resolve_body returns None.
    assert!(resolve_body("r2e-core").is_none());
}

#[test]
fn unknown_module_does_not_resolve() {
    assert!(resolve_body("nope").is_none());
}

// ── run() integration (Ok/Err surface) ──────────────────────────────

#[test]
fn run_list_ok() {
    assert!(docs::run(None, false, false).is_ok());
}

#[test]
fn run_known_slug_ok() {
    assert!(docs::run(Some("events"), false, false).is_ok());
}

#[test]
fn run_known_slug_full_ok() {
    assert!(docs::run(Some("events"), true, false).is_ok());
}

#[test]
fn run_known_slug_pretty_ok() {
    assert!(docs::run(Some("events"), false, true).is_ok());
}

#[test]
fn run_crate_alias_one_to_one_ok() {
    assert!(docs::run(Some("r2e-events"), false, false).is_ok());
}

#[test]
fn run_crate_alias_many_lists_ok() {
    // `r2e-core` owns several modules → lists them, still Ok.
    assert!(docs::run(Some("r2e-core"), false, false).is_ok());
}

#[test]
fn run_unknown_module_errors() {
    let err = docs::run(Some("nope"), false, false).unwrap_err();
    assert!(err.to_string().contains("unknown module"));
}
