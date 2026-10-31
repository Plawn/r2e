use r2e_tracing::tracing_subscriber::fmt::format::FmtSpan;
use r2e_tracing::{LogFormat, SpanEvents, TracingOptions};
use serde::Deserialize;

#[test]
fn default_has_expected_values() {
    let opts = TracingOptions::default();
    assert_eq!(opts, TracingOptions::new("info"));
    assert_eq!(opts.filter, "info");
    assert!(opts.format.is_none());
    assert!(opts.target.is_none());
    assert!(opts.thread_ids.is_none());
    assert!(opts.thread_names.is_none());
    assert!(opts.file.is_none());
    assert!(opts.line_number.is_none());
    assert!(opts.level.is_none());
    assert!(opts.ansi.is_none());
    assert!(opts.span_events.is_none());
}

#[test]
fn effective_defaults() {
    let opts = TracingOptions::default();
    assert_eq!(opts.effective_format(), LogFormat::Pretty);
    assert_eq!(opts.effective_span_events(), FmtSpan::CLOSE);
}

#[test]
fn builder_methods() {
    let opts = TracingOptions::default()
        .with_format(LogFormat::Json)
        .with_filter("trace")
        .with_target(false)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true)
        .with_level(false)
        .with_ansi(false)
        .with_span_events(SpanEvents::Full);

    assert_eq!(opts.format, Some(LogFormat::Json));
    assert_eq!(opts.effective_format(), LogFormat::Json);
    assert_eq!(opts.filter, "trace");
    assert_eq!(opts.target, Some(false));
    assert_eq!(opts.thread_ids, Some(true));
    assert_eq!(opts.thread_names, Some(true));
    assert_eq!(opts.file, Some(true));
    assert_eq!(opts.line_number, Some(true));
    assert_eq!(opts.level, Some(false));
    assert_eq!(opts.ansi, Some(false));
    assert_eq!(opts.span_events, Some(SpanEvents::Full));
    assert_eq!(opts.effective_span_events(), FmtSpan::FULL);
}

#[test]
fn log_format_deserializes_lowercase() {
    let de = |s: &str| {
        LogFormat::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(s))
    };
    assert_eq!(de("json").unwrap(), LogFormat::Json);
    assert_eq!(de("pretty").unwrap(), LogFormat::Pretty);
    assert!(de("neon").is_err());
}
