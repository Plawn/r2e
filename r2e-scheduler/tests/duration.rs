use std::time::Duration;

use r2e_scheduler::parse_duration;

#[test]
fn parses_simple_suffixes() {
    assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
    assert_eq!(parse_duration("5s").unwrap(), Duration::from_secs(5));
    assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
    assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
    assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86_400));
}

#[test]
fn parses_compound_segments() {
    assert_eq!(parse_duration("1h30m").unwrap(), Duration::from_secs(5400));
    assert_eq!(parse_duration("2m30s").unwrap(), Duration::from_secs(150));
    assert_eq!(
        parse_duration("1h15m30s").unwrap(),
        Duration::from_secs(4530)
    );
}

#[test]
fn trims_whitespace() {
    assert_eq!(parse_duration(" 5s ").unwrap(), Duration::from_secs(5));
}

#[test]
fn rejects_invalid_input() {
    assert!(parse_duration("").is_err());
    assert!(parse_duration("abc").is_err());
    assert!(parse_duration("5x").is_err());
    assert!(parse_duration("30").is_err(), "bare number needs a suffix");
    assert!(parse_duration("0s").is_err(), "zero duration rejected");
}
