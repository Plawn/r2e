//! Integration tests for the duration-string parser used by `#[scheduled]`.
//!
//! `r2e-macros` is a proc-macro crate, so its `pub fn` items are not reachable
//! from external crates. We pull the pure-function module in via `#[path]`.

#[path = "../src/extract/duration.rs"]
mod duration;

use duration::parse_duration_ms;

#[test]
fn parse_seconds() {
    assert_eq!(parse_duration_ms("30s").unwrap(), 30_000);
}

#[test]
fn parse_minutes() {
    assert_eq!(parse_duration_ms("5m").unwrap(), 300_000);
}

#[test]
fn parse_hours() {
    assert_eq!(parse_duration_ms("2h").unwrap(), 7_200_000);
}

#[test]
fn parse_days() {
    assert_eq!(parse_duration_ms("1d").unwrap(), 86_400_000);
}

#[test]
fn parse_millis() {
    assert_eq!(parse_duration_ms("500ms").unwrap(), 500);
}

#[test]
fn parse_combined() {
    assert_eq!(parse_duration_ms("1h30m").unwrap(), 5_400_000);
    assert_eq!(parse_duration_ms("2m30s").unwrap(), 150_000);
    assert_eq!(parse_duration_ms("1h15m30s").unwrap(), 4_530_000);
}

#[test]
fn parse_zero_rejected() {
    assert!(parse_duration_ms("0s").is_err());
}

#[test]
fn parse_no_unit_rejected() {
    assert!(parse_duration_ms("30").is_err());
}

#[test]
fn parse_empty_rejected() {
    assert!(parse_duration_ms("").is_err());
}

#[test]
fn parse_unknown_suffix_rejected() {
    assert!(parse_duration_ms("5x").is_err());
}
