//! The origin allowlist grammar shared by `cors.*` and the MCP allowlists:
//! `layers::{validate_origin, origin_allowed, allow_origins}`.

use r2e_http::layers::{origin_allowed, validate_origin};

fn list(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|s| s.to_string()).collect()
}

// ── validate_origin ────────────────────────────────────────────────────────

#[test]
fn accepts_the_four_forms() {
    for ok in [
        "*",
        "https://app.example.com",
        "http://localhost:5173",
        "http://localhost:*",
        "https://[::1]:3000",
        "https://[::1]",
        "chrome-extension://abcdefghijklmnop",
    ] {
        assert_eq!(validate_origin(ok), Ok(()), "{ok}");
    }
}

#[test]
fn rejects_missing_scheme() {
    let err = validate_origin("app.example.com").unwrap_err();
    assert!(err.contains("\"app.example.com\""), "{err}");
    assert!(err.contains("scheme://host"), "{err}");
}

#[test]
fn rejects_path_query_fragment_and_spaces() {
    for bad in [
        "http://app.example.com/",
        "http://app.example.com/api",
        "http://app.example.com?x=1",
        "http://app.example.com#top",
        "http://app example.com",
    ] {
        let err = validate_origin(bad).unwrap_err();
        assert!(err.contains("no path"), "{bad}: {err}");
    }
}

#[test]
fn rejects_empty_host_and_bad_port() {
    assert!(validate_origin("http://").unwrap_err().contains("host is empty"));
    assert!(validate_origin("http://:3000").unwrap_err().contains("host is empty"));
    assert!(validate_origin("http://localhost:abc").unwrap_err().contains("port"));
    assert!(validate_origin("http://localhost:").unwrap_err().contains("port"));
    assert!(validate_origin("1http://x").unwrap_err().contains("scheme"));
    assert!(validate_origin("https://[::1").unwrap_err().contains("IPv6"));
}

// ── origin_allowed ─────────────────────────────────────────────────────────

#[test]
fn exact_entries_match_exactly() {
    let allowed = list(&["http://localhost:5173", "https://app.example.com"]);
    assert!(origin_allowed(&allowed, "http://localhost:5173"));
    assert!(origin_allowed(&allowed, "https://app.example.com"));
    assert!(!origin_allowed(&allowed, "http://localhost:5174"));
    assert!(!origin_allowed(&allowed, "http://app.example.com"));
    assert!(!origin_allowed(&allowed, "https://app.example.com.evil.io"));
}

#[test]
fn port_wildcard_matches_any_numeric_port_only() {
    let allowed = list(&["http://localhost:*"]);
    assert!(origin_allowed(&allowed, "http://localhost:5173"));
    assert!(origin_allowed(&allowed, "http://localhost:80"));
    assert!(!origin_allowed(&allowed, "http://localhost"), "a port is required");
    assert!(!origin_allowed(&allowed, "http://localhost:"), "empty port");
    assert!(!origin_allowed(&allowed, "http://localhost:51x3"));
    assert!(!origin_allowed(&allowed, "http://localhost.evil.io:5173"));
    assert!(!origin_allowed(&allowed, "https://localhost:5173"), "scheme is part of the prefix");
}

#[test]
fn star_matches_everything_and_empty_list_matches_nothing() {
    assert!(origin_allowed(&list(&["*"]), "http://anything.example"));
    assert!(!origin_allowed(&[], "http://anything.example"));
}
