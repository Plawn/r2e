use r2e_tracing::tracing_subscriber::fmt::format::FmtSpan;
use r2e_tracing::SpanEvents;
use serde::Deserialize;

#[test]
fn span_events_to_fmt_span() {
    assert_eq!(SpanEvents::None.to_fmt_span(), FmtSpan::NONE);
    assert_eq!(SpanEvents::New.to_fmt_span(), FmtSpan::NEW);
    assert_eq!(SpanEvents::Close.to_fmt_span(), FmtSpan::CLOSE);
    assert_eq!(SpanEvents::Active.to_fmt_span(), FmtSpan::ACTIVE);
    assert_eq!(SpanEvents::Full.to_fmt_span(), FmtSpan::FULL);
}

#[test]
fn span_events_deserializes_lowercase() {
    for (input, expected) in [
        ("none", SpanEvents::None),
        ("new", SpanEvents::New),
        ("close", SpanEvents::Close),
        ("active", SpanEvents::Active),
        ("full", SpanEvents::Full),
    ] {
        let de = serde::de::value::StrDeserializer::<serde::de::value::Error>::new(input);
        assert_eq!(
            SpanEvents::deserialize(de).unwrap(),
            expected,
            "input: {input}"
        );
    }
}

#[test]
fn default_is_close() {
    assert_eq!(SpanEvents::default(), SpanEvents::Close);
}
