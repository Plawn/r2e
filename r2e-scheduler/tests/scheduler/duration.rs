use std::time::Duration;

use r2e_scheduler::{parse_duration, PositiveDuration};

#[test]
fn positive_duration_rejects_zero_and_wraps_nonzero() {
    // Zero is unrepresentable, whatever the constructor.
    assert_eq!(PositiveDuration::new(Duration::ZERO), None);
    assert_eq!(PositiveDuration::from_millis(0), None);
    assert_eq!(PositiveDuration::from_secs(0), None);

    // Non-zero values round-trip through `get()` / `From` / `Deref`.
    let d = PositiveDuration::from_secs(5).unwrap();
    assert_eq!(d.get(), Duration::from_secs(5));
    assert_eq!(Duration::from(d), Duration::from_secs(5));
    assert_eq!(d.as_secs(), 5); // via Deref to Duration
    assert_eq!(
        PositiveDuration::new(Duration::from_millis(250))
            .unwrap()
            .get(),
        Duration::from_millis(250)
    );
}

#[test]
fn positive_duration_display_matches_underlying_duration() {
    let d = PositiveDuration::from_millis(1500).unwrap();
    assert_eq!(d.to_string(), format!("{:?}", Duration::from_millis(1500)));
}

#[test]
fn parses_simple_suffixes() {
    assert_eq!(
        parse_duration("500ms").unwrap().get(),
        Duration::from_millis(500)
    );
    assert_eq!(parse_duration("5s").unwrap().get(), Duration::from_secs(5));
    assert_eq!(
        parse_duration("2m").unwrap().get(),
        Duration::from_secs(120)
    );
    assert_eq!(
        parse_duration("1h").unwrap().get(),
        Duration::from_secs(3600)
    );
    assert_eq!(
        parse_duration("1d").unwrap().get(),
        Duration::from_secs(86_400)
    );
}

#[test]
fn parses_compound_segments() {
    assert_eq!(
        parse_duration("1h30m").unwrap().get(),
        Duration::from_secs(5400)
    );
    assert_eq!(
        parse_duration("2m30s").unwrap().get(),
        Duration::from_secs(150)
    );
    assert_eq!(
        parse_duration("1h15m30s").unwrap().get(),
        Duration::from_secs(4530)
    );
}

#[test]
fn trims_whitespace() {
    assert_eq!(
        parse_duration(" 5s ").unwrap().get(),
        Duration::from_secs(5)
    );
}

#[test]
fn rejects_invalid_input() {
    assert!(parse_duration("").is_err());
    assert!(parse_duration("abc").is_err());
    assert!(parse_duration("5x").is_err());
    assert!(parse_duration("30").is_err(), "bare number needs a suffix");
    assert!(parse_duration("0s").is_err(), "zero duration rejected");
}

#[test]
fn rejects_number_that_overflows_u64() {
    // 30 nines overflow u64 → the `current_num.parse()` map_err error path.
    let huge = format!("{}s", "9".repeat(30));
    let err = parse_duration(&huge).unwrap_err();
    assert!(err.contains("invalid number"), "got: {err}");
}

#[test]
fn rejects_value_that_overflows_when_multiplied_by_its_unit() {
    // Fits in u64 as a bare number, but `num * multiplier` overflows: this must
    // return an error, not panic (debug) or silently wrap (release).
    let err = parse_duration("10000000000000000d").unwrap_err();
    assert!(err.contains("too large"), "got: {err}");
    // Accumulation across segments can also overflow.
    let err = parse_duration("18446744073709551d1d").unwrap_err();
    assert!(err.contains("too large"), "got: {err}");
}

#[test]
fn rejects_unexpected_character() {
    // A leading char that is neither a digit nor alphabetic hits the
    // "unexpected character" branch.
    let err = parse_duration("@").unwrap_err();
    assert!(err.contains("unexpected character"), "got: {err}");
    // Also after a valid segment: "5s#".
    assert!(parse_duration("5s#").is_err());
}
